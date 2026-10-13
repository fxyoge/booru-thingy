mod api;
mod auth;
mod config;
mod db;
mod error;
#[cfg(test)]
mod integration;
mod media;
mod queue;
mod search;
mod storage;
mod uploads;
mod web;
mod worker;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use clap::Parser;
use config::{Cli, Command, Config};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub storage: storage::Storage,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli)?;
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&config.rust_log)
                .map_err(|_| anyhow::anyhow!("invalid RUST_LOG / rust_log"))?,
        )
        .init();
    let db = PgPoolOptions::new()
        .max_connections(if cli.command == Some(Command::Migrate) {
            1
        } else {
            app_connection_limit(&config) as u32
        })
        .connect(&config.database_url)
        .await?;
    if matches!(cli.command, Some(Command::Run | Command::Migrate)) {
        db::MIGRATOR.run(&db).await?;
    }
    if cli.command == Some(Command::Migrate) {
        return Ok(());
    }
    let app = App {
        storage: storage::Storage::new(&config)?,
        config,
        db,
    };
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        shutdown.cancel();
    });
    let server = async {
        if cli.command != Some(Command::Worker) {
            let listener = tokio::net::TcpListener::bind(app.config.listen_addr).await?;
            tracing::info!(address = %app.config.listen_addr, "server listening");
            axum::serve(listener, router(app.clone()))
                .with_graceful_shutdown(stop.clone().cancelled_owned())
                .await?;
            stop.cancel();
        } else {
            stop.cancelled().await;
        }
        Ok::<_, anyhow::Error>(())
    };
    let workers = async {
        if cli.command != Some(Command::Serve) {
            queue::run(app.clone(), stop.clone()).await?;
        } else {
            stop.cancelled().await;
        }
        Ok::<_, anyhow::Error>(())
    };
    // Both the server and worker runtime are owned by this scope.
    let outcome = tokio::try_join!(server, workers);
    stop.cancel();
    app.db.close().await;
    outcome?;
    Ok(())
}

fn app_connection_limit(config: &Config) -> usize {
    // Media work is outside transactions; publication and API writes are short.
    (config.worker_concurrency + 8).max(12)
}

pub fn router(app: App) -> Router {
    let protected = api::routes()
        .merge(uploads::routes())
        .route("/session", get(auth::session).delete(auth::logout))
        .route_layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_auth,
        ));
    Router::new()
        .route("/", get(web::home))
        .route("/uploads/new", get(web::upload_page))
        .route("/assets/{*path}", get(web::asset))
        .route("/login", post(auth::login))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(web::ready))
        .merge(protected)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), web::headers))
        .with_state(app)
}
