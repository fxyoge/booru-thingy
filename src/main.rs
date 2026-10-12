mod api;
mod auth;
mod config;
mod error;
#[cfg(test)]
mod integration;
mod media;
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
use config::{Command, Config};
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
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "booru_thingy=info".into()),
        )
        .init();
    let config = Config::parse().validate()?;
    let db = PgPoolOptions::new()
        .max_connections((app_connection_limit(&config)) as u32)
        .connect(&config.database_url)
        .await?;
    if config.command.is_none() || config.command == Some(Command::Migrate) {
        sqlx::migrate!().run(&db).await?;
    }
    if config.command == Some(Command::Migrate) {
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
    let worker = if app.config.command != Some(Command::Serve) {
        Some(tokio::spawn(worker::run(app.clone(), stop.clone())))
    } else {
        None
    };
    if app.config.command != Some(Command::Worker) {
        let listener = tokio::net::TcpListener::bind(app.config.listen_addr).await?;
        tracing::info!(address = %app.config.listen_addr, "server listening");
        axum::serve(listener, router(app.clone()))
            .with_graceful_shutdown(stop.clone().cancelled_owned())
            .await?;
        stop.cancel();
    } else {
        stop.cancelled().await;
    }
    if let Some(worker) = worker {
        worker.await??;
    }
    app.db.close().await;
    Ok(())
}

fn app_connection_limit(config: &Config) -> usize {
    // Processing holds a transaction; leave room for heartbeats and API traffic.
    (config.worker_concurrency * 2 + 8).max(12)
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
        .route("/assets/app.css", get(web::css))
        .route("/assets/app.js", get(web::js))
        .route("/login", post(auth::login))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(web::ready))
        .merge(protected)
        .layer(DefaultBodyLimit::max(
            (app.config.max_upload_bytes as usize).saturating_add(1024 * 1024),
        ))
        .layer(middleware::from_fn(web::headers))
        .with_state(app)
}
