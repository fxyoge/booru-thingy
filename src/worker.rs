//! A checkpointed, at-least-once media pipeline. Apalis owns all execution state.
use crate::{
    App, api,
    db::{generated as queries, optional},
    media,
};
use apalis::{
    layers::retry::{
        HasherRng, RetryPolicy,
        backoff::{ExponentialBackoffMaker, MakeBackoff},
    },
    prelude::*,
};
use apalis_postgres::{Config as QueueConfig, PostgresStorage};
use apalis_workflow::SteppedFlow;
use apalis_workflow::sequential::{AndThen, WorkflowContext};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use std::{net::IpAddr, path::Path, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

const QUEUE: &str = "uploads-v1";
const ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Input {
    item_id: i64,
    run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Prepared {
    #[serde(flatten)]
    input: Input,
    metadata: media::Metadata,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Produced {
    #[serde(flatten)]
    prepared: Prepared,
    variants: Vec<media::Variant>,
}

/// Domain acceptance and enqueueing commit together; there is no outbox/dispatcher.
pub async fn enqueue(tx: &mut Transaction<'_, Postgres>, item_id: i64) -> anyhow::Result<()> {
    let input = Input {
        item_id,
        run_id: uuid::Uuid::new_v4().to_string(),
    };
    // A manual retry starts at the furthest failed step using Apalis's stored
    // input. In particular, publication retries don't regenerate previews.
    let failed = optional(queries::get_failed_upload_task(&mut *tx, item_id).await)?;
    let (mut payload, step_index) = if let Some(task) = failed {
        (
            serde_json::from_slice::<serde_json::Value>(&task.job)?,
            task.step_index as usize,
        )
    } else {
        (serde_json::to_value(&input)?, 0)
    };
    payload["run_id"] = input.run_id.clone().into();
    queries::start_upload_execution(
        &mut *tx,
        queries::StartUploadExecutionParams {
            id: item_id,
            run_id: Some(&input.run_id),
        },
    )
    .await?;
    let task = TaskBuilder::new(serde_json::to_vec(&payload)?)
        .max_attempts(ATTEMPTS)
        .metadata(&WorkflowContext { step_index })
        .build();
    apalis_postgres::queries::push_tasks(&mut **tx, QUEUE, vec![task]).await?;
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
                        PostgresStorage::<Input>::new(&app.db)
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
                    .retry(
                        RetryPolicy::retries(ATTEMPTS - 1)
                            .with_backoff(backoff)
                            .retry_if(|error: &BoxDynError| !error.is::<AbortError>()),
                    )
                    .build({
                        // These layers belong INSIDE the workflow so Apalis's
                        // lifecycle records timeouts and exhausted interruptions.
                        let deadline = Duration::from_secs(app.config.process_timeout_seconds);
                        SteppedFlow::new(QUEUE)
                            .add_step::<_, Prepared>(AndThen::new(
                                tower::ServiceBuilder::new()
                                    .map_err(sanitize)
                                    .filter(budget::<Input>)
                                    .timeout(deadline)
                                    .service(task_fn(prepare)),
                            ))
                            .add_step::<_, Produced>(AndThen::new(
                                tower::ServiceBuilder::new()
                                    .map_err(sanitize)
                                    .filter(budget::<Prepared>)
                                    .timeout(deadline)
                                    .service(task_fn(produce)),
                            ))
                            .add_step::<_, ()>(AndThen::new(
                                tower::ServiceBuilder::new()
                                    .map_err(sanitize)
                                    .filter(budget::<Produced>)
                                    .timeout(deadline)
                                    .service(task_fn(publish)),
                            ))
                    })
            }
        })
        .register(move |_| {
            WorkerBuilder::new(format!("maintenance-{process_id}"))
                .backend(apalis_cron::CronScheduler::new(
                    "0 0 * * * *".parse::<cron::Schedule>().unwrap(),
                ))
                .data(app.clone())
                .build(|_: apalis_cron::Tick, app: Data<App>| async move {
                    cleanup(&app).await.map_err(|e| -> BoxDynError { e.into() })
                })
        })
        .run_with_signal(async move {
            stop.cancelled().await;
            Ok::<_, std::io::Error>(())
        })
        .await?;
    Ok(())
}

fn budget<T>(task: Task<T>) -> Result<Task<T>, AbortError> {
    // Apalis also increments attempts when rescuing a dead worker's task.
    if task.attempt() >= ATTEMPTS {
        Err(AbortError::new(std::io::Error::other(
            "automatic retry limit reached",
        )))
    } else {
        Ok(task)
    }
}

fn sanitize(error: BoxDynError) -> BoxDynError {
    if error.is::<AbortError>() {
        error
    } else {
        std::io::Error::other(safe_error(error.as_ref())).into()
    }
}

fn safe_error(error: &dyn std::error::Error) -> String {
    let message = error.to_string();
    // URL fetch errors are intentionally generated without including the URL.
    if message.contains("http://") || message.contains("https://") {
        "media operation failed".into()
    } else {
        message.chars().take(300).collect()
    }
}
async fn prepare(input: Input, app: Data<App>) -> Result<Prepared, BoxDynError> {
    let row = queries::get_processing_item(&app.db, input.item_id).await?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("input");
    if let Some(key) = row.original_key.as_ref().or(row.staging_key.as_ref()) {
        app.storage
            .download(key, &path, app.config.max_upload_bytes)
            .await?;
    } else {
        fetch(&app, &row.source_url, &path).await?;
    }
    let metadata = media::inspect(&path).await?;
    let key = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
    if row.original_key.is_none() {
        app.storage.put(&key, &path, &metadata.content_type).await?;
        let remembered = queries::remember_original(
            &app.db,
            queries::RememberOriginalParams {
                id: input.item_id,
                run_id: Some(&input.run_id),
                original_key: Some(&key),
            },
        )
        .await?;
        if remembered.original_key.as_deref() != Some(&key) {
            return Err("input was already accepted; retry using the accepted original".into());
        }
    }
    Ok(Prepared { input, metadata })
}

async fn produce(prepared: Prepared, app: Data<App>) -> Result<Produced, BoxDynError> {
    let metadata = &prepared.metadata;
    let existing = optional(queries::get_asset_by_hash(&app.db, &metadata.sha256).await)?;
    let variants = if existing.is_some_and(|a| a.status == "active") {
        Vec::new()
    } else {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("input");
        let key = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
        app.storage
            .download(&key, &path, app.config.max_upload_bytes)
            .await?;
        media::variants(&app, metadata, &path, directory.path()).await?
    };
    Ok(Produced { prepared, variants })
}

async fn publish(produced: Produced, app: Data<App>) -> Result<(), BoxDynError> {
    let Produced {
        prepared: Prepared { input, metadata },
        variants,
    } = produced;
    let mut tx = app.db.begin().await?;
    let row = queries::lock_processing_item(&mut tx, input.item_id).await?;
    // Publication outcome is immutable per item, including its duplicate flag.
    // Older executions can store hash-addressed bytes, but cannot publish.
    if row.post_id.is_some() || row.run_id.as_deref() != Some(&input.run_id) {
        return Ok(());
    }
    let asset = optional(queries::get_asset_by_hash(&mut tx, &metadata.sha256).await)?;
    let asset_id: i64 = if let Some(asset) = asset.as_ref().filter(|r| r.status == "active") {
        asset.id
    } else {
        let original = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
        let id = queries::upsert_media_asset(
            &mut tx,
            queries::UpsertMediaAssetParams {
                sha_256: &metadata.sha256,
                md_5: &metadata.md5,
                file_ext: &metadata.ext,
                content_type: &metadata.content_type,
                file_size: metadata.size,
                image_width: metadata.width,
                image_height: metadata.height,
                duration: metadata.duration,
                original_key: &original,
            },
        )
        .await?
        .id;
        queries::insert_media_variants(
            &mut tx,
            queries::InsertMediaVariantsParams {
                media_asset_id: id,
                variants: serde_json::to_value(variants)?,
            },
        )
        .await?;
        id
    };
    let tags = api::normalize_tags(&row.tag_string).map_err(|e| anyhow::anyhow!(e.1))?;
    let (post_id, duplicate) = api::publish(&mut tx, asset_id, &row.source, &tags, &row.rating)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    queries::complete_upload_item(
        &mut tx,
        queries::CompleteUploadItemParams {
            id: input.item_id,
            media_asset_id: Some(asset_id),
            post_id: Some(post_id),
            duplicate,
        },
    )
    .await?;
    queries::touch_upload(&mut tx, row.upload_id).await?;
    tx.commit().await?;
    tracing::info!(
        item_id = input.item_id,
        post_id,
        duplicate,
        "upload published"
    );
    Ok(())
}

pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_multicast()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && a != 0
                && a < 240
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                public_ip(IpAddr::V4(ip))
            } else {
                let segments = ip.segments();
                (segments[0] & 0xe000) == 0x2000
                    && !(segments[0] == 0x2001 && matches!(segments[1], 0 | 2 | 0xdb8))
                    && segments[0] != 0x2002
            }
        }
    }
}
pub async fn fetch(app: &App, source: &str, path: &Path) -> anyhow::Result<()> {
    let mut url = url::Url::parse(source).map_err(|_| anyhow::anyhow!("invalid source URL"))?;
    for _ in 0..6 {
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none(),
            "invalid source URL"
        );
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("source URL has no host"))?
            .trim_matches(['[', ']']);
        let port = url.port_or_known_default().unwrap();
        let addresses: Vec<_> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| anyhow::anyhow!("source hostname lookup failed"))?
            .collect();
        anyhow::ensure!(!addresses.is_empty(), "source hostname has no addresses");
        anyhow::ensure!(
            app.config.allow_private_source_urls || addresses.iter().all(|a| public_ip(a.ip())),
            "source address is not public"
        );
        // Pin validated DNS answers and disable proxies to prevent rebinding/bypass.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(app.config.process_timeout_seconds))
            .build()?;
        let response = client
            .get(url.clone())
            .header("User-Agent", "booru-thingy/0.1")
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("source download failed"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("source redirect has no location"))?;
            url = url
                .join(location)
                .map_err(|_| anyhow::anyhow!("invalid source redirect"))?;
            continue;
        }
        anyhow::ensure!(
            response.status().is_success(),
            "source returned HTTP {}",
            response.status().as_u16()
        );
        anyhow::ensure!(
            response.content_length().unwrap_or(0) <= app.config.max_upload_bytes,
            "source exceeds upload limit"
        );
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(path).await?;
        let mut size = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow::anyhow!("source download interrupted"))?;
            size += chunk.len() as u64;
            anyhow::ensure!(
                size <= app.config.max_upload_bytes,
                "source exceeds upload limit"
            );
            file.write_all(&chunk).await?;
        }
        anyhow::ensure!(size > 0, "source returned an empty file");
        return Ok(());
    }
    anyhow::bail!("too many source redirects")
}
pub(crate) async fn cleanup(app: &App) -> anyhow::Result<()> {
    queries::delete_expired_sessions(&app.db).await?;
    queries::expire_interrupted_items(&app.db).await?;
    for grant in queries::expired_upload_grants(&app.db).await? {
        // Keep capabilities tracked until expiry + grace: deleting sooner allows replay
        // to recreate an object after its tracking record has disappeared.
        app.storage.delete(&grant.object_key).await?;
        queries::delete_upload_grant(&app.db, grant.id).await?;
    }
    for snapshot in queries::unused_upload_snapshots(&app.db).await? {
        app.storage.delete(&snapshot.object_key).await?;
        queries::delete_upload_snapshot(&app.db, snapshot.id).await?;
    }
    let rows = queries::list_staging_cleanup_candidates(&app.db).await?;
    for row in rows {
        let mut tx = app.db.begin().await?;
        let key = optional(queries::lock_staging_cleanup_item(&mut tx, row.id).await)?
            .and_then(|row| row.staging_key);
        if let Some(key) = key {
            app.storage.delete(&key).await?;
            queries::clear_staging_key(&mut tx, row.id).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn app(pool: sqlx::PgPool) -> App {
        let config = std::sync::Arc::new(crate::config::Config {
            s3_endpoint: "http://127.0.0.1:9".into(),
            s3_bucket: "unused".into(),
            aws_access_key_id: "test-access".into(),
            aws_secret_access_key: "test-secret".into(),
            allow_private_source_urls: true,
            process_timeout_seconds: 2,
            ..crate::config::Config::default()
        });
        App {
            storage: crate::storage::Storage::new(&config).unwrap(),
            config,
            db: pool,
        }
    }

    async fn source_item(pool: &sqlx::PgPool, source: &str) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let upload_id = queries::create_upload(&mut tx).await.unwrap().id;
        let item = queries::create_source_item(
            &mut tx,
            queries::CreateSourceItemParams {
                upload_id,
                source_url: source,
                filename: "image.png",
            },
        )
        .await
        .unwrap()
        .id;
        enqueue(&mut tx, item).await.unwrap();
        tx.commit().await.unwrap();
        item
    }

    async fn produced(pool: &sqlx::PgPool, item_id: i64) -> Produced {
        Produced {
            prepared: Prepared {
                input: Input {
                    item_id,
                    run_id: queries::get_processing_item(pool, item_id)
                        .await
                        .unwrap()
                        .run_id
                        .unwrap(),
                },
                metadata: media::Metadata {
                    sha256: "same-hash".into(),
                    md5: "same-md5".into(),
                    ext: "png".into(),
                    content_type: "image/png".into(),
                    size: 100,
                    width: 64,
                    height: 48,
                    duration: None,
                },
            },
            variants: Vec::new(),
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    #[ignore = "requires PostgreSQL"]
    async fn apalis_retries_exhaust_and_status_survives_restart(pool: sqlx::PgPool) {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
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
        let item = source_item(&pool, &source).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let state = queries::get_upload_item_state(&pool, item).await.unwrap();
                if state.status == "error" {
                    assert!(state.error.contains("HTTP 500"));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), ATTEMPTS);
        stop.cancel();
        worker.await.unwrap().unwrap();
        assert_eq!(
            queries::get_upload_item_state(&pool, item)
                .await
                .unwrap()
                .status,
            "error"
        );

        // A new execution gets a fresh library budget. The previous terminal
        // task remains history, and cannot make the new execution look failed.
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
        // Model repeated process deaths spending the final attempt. Apalis's
        // orphan recovery must finish this as an error without fetching again.
        sqlx::query(
            "INSERT INTO apalis.workers(id,worker_type,storage_name,last_seen)
            VALUES('oom-worker','uploads-v1','PgStorage',now()-interval '1 hour')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE apalis.jobs SET status='Running',lock_by='oom-worker',attempts=$2
            WHERE upload_task_run(job)=(SELECT run_id FROM upload_items WHERE id=$1)",
        )
        .bind(item)
        .bind((ATTEMPTS - 1) as i32)
        .execute(&pool)
        .await
        .unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
        tokio::time::timeout(Duration::from_secs(15), async {
            while queries::get_upload_item_state(&pool, item)
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
        assert_eq!(requests.load(Ordering::SeqCst), ATTEMPTS);
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
    async fn publication_converges_and_replay_preserves_outcome(pool: sqlx::PgPool) {
        let app = app(pool.clone());
        let one = source_item(&pool, "https://example.com/one").await;
        let two = source_item(&pool, "https://example.com/two").await;
        let a = produced(&pool, one).await;
        let b = produced(&pool, two).await;
        let (left, right) = tokio::join!(
            publish(a.clone(), Data::new(app.clone())),
            publish(b.clone(), Data::new(app.clone()))
        );
        left.unwrap();
        right.unwrap();
        let rows: Vec<(i64, bool)> =
            sqlx::query_as("SELECT post_id,duplicate FROM upload_items ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows[0].0, rows[1].0);
        assert_ne!(rows[0].1, rows[1].1);
        publish(a, Data::new(app.clone())).await.unwrap();
        publish(b, Data::new(app)).await.unwrap();
        let replayed: Vec<(i64, bool)> =
            sqlx::query_as("SELECT post_id,duplicate FROM upload_items ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, replayed);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_assets")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM posts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    #[ignore = "requires PostgreSQL"]
    async fn duplicate_successors_and_manual_retry_use_publication_checkpoint(pool: sqlx::PgPool) {
        let item = source_item(
            &pool,
            "https://example.invalid/input-is-no-longer-available",
        )
        .await;
        let output = produced(&pool, item).await;
        // Model a completed parent, with two successors from a crash during the
        // send/ack gap. The pipeline must converge even with this duplicate work.
        sqlx::raw_sql(
            "UPDATE apalis.jobs SET status='Done';
            ALTER TABLE posts ADD CONSTRAINT reject_publish CHECK(false) NOT VALID",
        )
        .execute(&pool)
        .await
        .unwrap();
        let task = || {
            TaskBuilder::new(serde_json::to_vec(&output).unwrap())
                .max_attempts(ATTEMPTS)
                .metadata(&WorkflowContext { step_index: 2 })
                .build()
        };
        let mut tx = pool.begin().await.unwrap();
        apalis_postgres::queries::push_tasks(&mut *tx, QUEUE, vec![task(), task()])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(app(pool.clone()), stop.clone()));
        tokio::time::timeout(Duration::from_secs(15), async {
            while queries::get_upload_item_state(&pool, item)
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
        sqlx::query("ALTER TABLE posts DROP CONSTRAINT reject_publish")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        enqueue(&mut tx, item).await.unwrap();
        tx.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            while queries::get_upload_item_state(&pool, item)
                .await
                .unwrap()
                .status
                != "completed"
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        // Fake storage and an unavailable source make completion possible ONLY
        // if the new execution starts at the persisted publication checkpoint.
        stop.cancel();
        worker.await.unwrap().unwrap();
        publish(output, Data::new(app(pool.clone()))).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM posts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[test]
    fn source_filter_blocks_internal_and_mapped_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.100.100.100",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(public_ip(
            "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap().into()
        ));
    }
}
