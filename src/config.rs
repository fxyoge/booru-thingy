use clap::{Parser, Subcommand};
use std::{net::SocketAddr, sync::Arc};

#[derive(Parser, Clone)]
#[command(version, about)]
pub struct Config {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,
    #[arg(long, env = "LISTEN_ADDR", default_value = "0.0.0.0:8080")]
    pub listen_addr: SocketAddr,
    #[arg(long, env = "BASE_URL", default_value = "http://localhost:8080")]
    pub base_url: String,
    #[arg(long, env = "OWNER_LOGIN", default_value = "owner")]
    pub owner_login: String,
    #[arg(long, env = "OWNER_API_KEY", hide_env_values = true)]
    pub owner_api_key: String,
    #[arg(long, env = "S3_ENDPOINT")]
    pub s3_endpoint: String,
    #[arg(long, env = "S3_PUBLIC_ENDPOINT")]
    pub s3_public_endpoint: Option<String>,
    #[arg(long, env = "S3_REGION", default_value = "us-east-1")]
    pub s3_region: String,
    #[arg(long, env = "S3_BUCKET")]
    pub s3_bucket: String,
    #[arg(long, env = "AWS_ACCESS_KEY_ID", hide_env_values = true)]
    pub s3_access_key: String,
    #[arg(long, env = "AWS_SECRET_ACCESS_KEY", hide_env_values = true)]
    pub s3_secret_key: String,
    #[arg(long, env = "S3_PATH_STYLE", default_value = "true", action = clap::ArgAction::Set)]
    pub s3_path_style: bool,
    #[arg(long, env = "MEDIA_URL_TTL_SECONDS", default_value = "3600")]
    pub media_url_ttl: u32,
    #[arg(long, env = "MAX_UPLOAD_BYTES", default_value = "536870912")]
    pub max_upload_bytes: u64,
    #[arg(long, env = "WORKER_CONCURRENCY", default_value = "2")]
    pub worker_concurrency: usize,
    #[arg(long, env = "PROCESS_TIMEOUT_SECONDS", default_value = "300")]
    pub process_timeout: u64,
    #[arg(long, env = "ALLOW_PRIVATE_SOURCE_URLS", default_value = "false", action = clap::ArgAction::Set)]
    pub allow_private_source_urls: bool,
}

#[derive(Subcommand, Debug, PartialEq, Clone)]
pub enum Command {
    Serve,
    Worker,
    Migrate,
}

impl Config {
    pub fn validate(self) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(
            !self.owner_api_key.is_empty(),
            "OWNER_API_KEY must not be empty"
        );
        anyhow::ensure!(
            !self.owner_login.is_empty(),
            "OWNER_LOGIN must not be empty"
        );
        anyhow::ensure!(
            (1..=604800).contains(&self.media_url_ttl),
            "media URL TTL must be 1..604800 seconds"
        );
        anyhow::ensure!(
            (1..=32).contains(&self.worker_concurrency),
            "worker concurrency must be 1..32"
        );
        anyhow::ensure!(
            self.max_upload_bytes > 0 && self.max_upload_bytes <= 5 * 1024 * 1024 * 1024,
            "upload limit must be between 1 byte and 5 GiB"
        );
        anyhow::ensure!(
            self.process_timeout > 0,
            "processing timeout must be positive"
        );
        let base = url::Url::parse(&self.base_url)?;
        anyhow::ensure!(
            matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
            "BASE_URL must be HTTP(S)"
        );
        anyhow::ensure!(
            base.path() == "/" && base.query().is_none() && base.fragment().is_none(),
            "BASE_URL must be an origin without a path"
        );
        Ok(Arc::new(self))
    }
}
