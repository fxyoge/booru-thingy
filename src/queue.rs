//! Apalis owns execution state; upload_items stores only the current task identity.
use crate::{App, db::generated as queries, worker};
use apalis::{
    layers::retry::{
        HasherRng, RetryPolicy,
        backoff::{ExponentialBackoffMaker, MakeBackoff},
    },
    prelude::*,
};
use apalis_postgres::{Config as QueueConfig, PostgresStorage};
use sqlx::{Postgres, Transaction};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const QUEUE: &str = "uploads";
const ATTEMPTS: usize = 3;

/// Acceptance, task identity, and enqueueing commit together.
pub async fn enqueue(tx: &mut Transaction<'_, Postgres>, item_id: i64) -> anyhow::Result<()> {
    let id = TaskId::from_ulid(ulid::Ulid::generate());
    let task = TaskBuilder::new(serde_json::to_vec(&item_id)?)
        .task_id(id.clone())
        .max_attempts(ATTEMPTS)
        .build();
    apalis_postgres::queries::push_tasks(&mut **tx, QUEUE, vec![task]).await?;
    queries::start_upload_execution(
        &mut *tx,
        queries::StartUploadExecutionParams {
            id: item_id,
            task_id: Some(&id.to_string()),
        },
    )
    .await?;
    Ok(())
}

pub async fn run(app: App, stop: CancellationToken) -> anyhow::Result<()> {
    let process_id = uuid::Uuid::new_v4();
    Monitor::new()
        .shutdown_timeout(Duration::from_secs(5))
        .should_restart(|worker, _| worker.restarts() < 3)
        .on_event(|worker, event| {
            if let Event::Error(error) = event {
                tracing::warn!(worker=worker.name(), %error, "background task failed");
            }
        })
        .register({
            let app = app.clone();
            move |restart| {
                let backoff = ExponentialBackoffMaker::new(
                    Duration::from_millis(250),
                    Duration::from_secs(2),
                    0.2,
                    HasherRng::default(),
                )
                .unwrap()
                .make_backoff();
                WorkerBuilder::new(format!("uploads-{process_id}-{restart}"))
                    .backend(
                        PostgresStorage::<i64>::new(&app.db)
                            .with_config(
                                QueueConfig::default()
                                    .queue(QUEUE)
                                    .batch_size(app.config.worker_concurrency)
                                    .heartbeat_interval(Duration::from_secs(15))
                                    .missed_heartbeats(2),
                            )
                            .with_pubsub()
                            .poll_with_interval(Duration::from_millis(500)),
                    )
                    .data(app.clone())
                    .concurrency(app.config.worker_concurrency)
                    .retry(RetryPolicy::retries(ATTEMPTS - 1).with_backoff(backoff))
                    .build(execute)
            }
        })
        .register(move |_| {
            WorkerBuilder::new(format!("maintenance-{process_id}"))
                .backend(apalis_cron::CronScheduler::new(
                    "0 * * * * *".parse::<cron::Schedule>().unwrap(),
                ))
                .data(app.clone())
                .build(|_: apalis_cron::Tick, app: Data<App>| async move {
                    worker::cleanup(&app)
                        .await
                        .map_err(|e| -> BoxDynError { e.into() })
                })
        })
        .run_with_signal(async move {
            stop.cancelled().await;
            Ok::<_, std::io::Error>(())
        })
        .await?;
    Ok(())
}

async fn execute(
    item_id: i64,
    app: Data<App>,
    id: TaskId,
    attempt: Attempt,
) -> Result<(), BoxDynError> {
    // Lifecycle has already counted this invocation. Orphan rescue also spends
    // the durable budget, so an exhausted rescued task must not do media work.
    if attempt.current() > ATTEMPTS {
        return Err(AbortError::new(std::io::Error::other("automatic retry limit reached")).into());
    }
    tokio::time::timeout(
        Duration::from_secs(app.config.process_timeout_seconds),
        worker::process(&app, item_id, &id.to_string()),
    )
    .await
    .map_err(|_| std::io::Error::other("upload processing timed out"))?
    .map_err(|error| std::io::Error::other(safe_error(&error)).into())
}

fn safe_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    if message.contains("http://") || message.contains("https://") {
        "media operation failed".into()
    } else {
        message.chars().take(300).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::Storage;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    pub(crate) fn app(pool: sqlx::PgPool) -> App {
        let config = Arc::new(crate::config::Config {
            s3_endpoint: "http://127.0.0.1:9".into(),
            s3_bucket: "unused".into(),
            aws_access_key_id: "test-access".into(),
            aws_secret_access_key: "test-secret".into(),
            allow_private_source_urls: true,
            process_timeout_seconds: 1,
            ..crate::config::Config::default()
        });
        App {
            storage: Storage::new(&config).unwrap(),
            config,
            db: pool,
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    #[ignore = "requires PostgreSQL"]
    async fn retries_manual_restart_and_orphan_exhaustion_share_one_budget(pool: sqlx::PgPool) {
        let requests = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}/image", listener.local_addr().unwrap());
        let routes = axum::Router::new().route(
            "/image",
            axum::routing::get({
                let requests = requests.clone();
                move || {
                    requests.fetch_add(1, Ordering::SeqCst);
                    async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        let mut tx = pool.begin().await.unwrap();
        let upload_id = queries::create_upload(&mut tx).await.unwrap().id;
        let item = queries::create_source_item(
            &mut tx,
            queries::CreateSourceItemParams {
                upload_id,
                source_url: &source,
                filename: "image.png",
            },
        )
        .await
        .unwrap()
        .id;
        enqueue(&mut tx, item).await.unwrap();
        tx.commit().await.unwrap();
        for expected_requests in [ATTEMPTS, ATTEMPTS * 2] {
            let stop = CancellationToken::new();
            let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
            await_error(&pool, item).await;
            assert_eq!(requests.load(Ordering::SeqCst), expected_requests);
            assert!(
                queries::get_upload_item_state(&pool, item)
                    .await
                    .unwrap()
                    .error
                    .contains("HTTP 500")
            );
            stop.cancel();
            worker.await.unwrap().unwrap();
            assert_eq!(
                queries::get_upload_item_state(&pool, item)
                    .await
                    .unwrap()
                    .status,
                "error"
            );
            let mut tx = pool.begin().await.unwrap();
            enqueue(&mut tx, item).await.unwrap();
            tx.commit().await.unwrap();
            assert_eq!(
                queries::get_upload_item_state(&pool, item)
                    .await
                    .unwrap()
                    .status,
                "pending"
            );
        }
        // Orphan rescue spends the final durable attempt before redelivery.
        sqlx::query(
            "INSERT INTO apalis.workers(id,worker_type,storage_name,last_seen)
            VALUES('oom-worker','uploads','PgStorage',now()-interval '1 hour')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE apalis.jobs SET status='Running',lock_by='oom-worker',attempts=$2
            WHERE id=(SELECT task_id FROM upload_items WHERE id=$1)",
        )
        .bind(item)
        .bind((ATTEMPTS - 1) as i32)
        .execute(&pool)
        .await
        .unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
        await_error(&pool, item).await;
        assert_eq!(requests.load(Ordering::SeqCst), ATTEMPTS * 2);
        assert!(
            queries::get_upload_item_state(&pool, item)
                .await
                .unwrap()
                .error
                .contains("retry limit")
        );
        stop.cancel();
        worker.await.unwrap().unwrap();
        server.abort();
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    #[ignore = "requires PostgreSQL"]
    async fn timeout_is_a_durable_failure_with_three_attempts(pool: sqlx::PgPool) {
        let requests = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}/slow", listener.local_addr().unwrap());
        let routes = axum::Router::new().route(
            "/slow",
            axum::routing::get({
                let requests = requests.clone();
                move || {
                    requests.fetch_add(1, Ordering::SeqCst);
                    async {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        "late response"
                    }
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        let mut tx = pool.begin().await.unwrap();
        let upload_id = queries::create_upload(&mut tx).await.unwrap().id;
        let item = queries::create_source_item(
            &mut tx,
            queries::CreateSourceItemParams {
                upload_id,
                source_url: &source,
                filename: "image.png",
            },
        )
        .await
        .unwrap()
        .id;
        enqueue(&mut tx, item).await.unwrap();
        tx.commit().await.unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
        await_error(&pool, item).await;
        assert_eq!(requests.load(Ordering::SeqCst), ATTEMPTS);
        assert!(
            queries::get_upload_item_state(&pool, item)
                .await
                .unwrap()
                .error
                .contains("timed out")
        );
        stop.cancel();
        worker.await.unwrap().unwrap();
        assert_eq!(
            queries::get_upload_item_state(&pool, item)
                .await
                .unwrap()
                .status,
            "error"
        );
        server.abort();
    }

    async fn await_error(pool: &sqlx::PgPool, item: i64) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while queries::get_upload_item_state(pool, item)
                .await
                .unwrap()
                .status
                != "error"
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }
}
