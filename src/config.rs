use anyhow::Context;
use clap::{Parser, Subcommand};
use confique::{Config as _, Layer};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Parser)]
#[command(
    version,
    about,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Merge this TOML file over the discovered files.
    #[arg(long, env = "BOORU_CONFIG", global = true, hide_env_values = true)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub overrides: ConfigLayer,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, PartialEq, Clone, Copy)]
pub enum Command {
    /// Apply migrations, then run the API and upload workers in one process.
    Run,
    /// Serve the API only
    Serve,
    /// Run upload workers only
    Worker,
    /// Apply database migrations and exit
    Migrate,
}

pub type ConfigLayer = <Config as confique::Config>::Layer;

#[derive(confique::Config, Clone)]
#[config(layer_attr(derive(clap::Args, Clone, serde::Serialize)))]
#[config(layer_attr(serde(deny_unknown_fields)))]
pub struct Config {
    // Empty defaults permit database-only migration; validate enforces requirements by mode.
    #[config(
        env = "DATABASE_URL",
        default = "",
        layer_attr(arg(long, global = true))
    )]
    pub database_url: String,
    /// HTTP bind address.
    #[config(
        env = "LISTEN_ADDR",
        default = "0.0.0.0:8080",
        layer_attr(arg(long, global = true))
    )]
    pub listen_addr: SocketAddr,
    /// External HTTP(S) origin, without a path.
    #[config(
        env = "BASE_URL",
        default = "http://localhost:8080",
        layer_attr(arg(long, global = true))
    )]
    pub base_url: String,
    #[config(
        env = "OWNER_LOGIN",
        default = "owner",
        layer_attr(arg(long, global = true))
    )]
    pub owner_login: String,
    #[config(
        env = "OWNER_API_KEY",
        default = "",
        layer_attr(arg(long, global = true))
    )]
    pub owner_api_key: String,
    #[config(
        env = "S3_ENDPOINT",
        default = "",
        layer_attr(arg(long, global = true))
    )]
    pub s3_endpoint: String,
    #[config(env = "S3_PUBLIC_ENDPOINT", layer_attr(arg(long, global = true)))]
    pub s3_public_endpoint: Option<String>,
    #[config(
        env = "S3_REGION",
        default = "us-east-1",
        layer_attr(arg(long, global = true))
    )]
    pub s3_region: String,
    #[config(env = "S3_BUCKET", default = "", layer_attr(arg(long, global = true)))]
    pub s3_bucket: String,
    #[config(
        env = "AWS_ACCESS_KEY_ID",
        default = "",
        layer_attr(arg(long, global = true))
    )]
    pub aws_access_key_id: String,
    #[config(
        env = "AWS_SECRET_ACCESS_KEY",
        default = "",
        layer_attr(arg(long, global = true))
    )]
    pub aws_secret_access_key: String,
    #[config(
        env = "S3_PATH_STYLE",
        default = true,
        layer_attr(arg(long, global = true))
    )]
    pub s3_path_style: bool,
    #[config(
        env = "MEDIA_URL_TTL_SECONDS",
        default = 3600,
        layer_attr(arg(long, global = true))
    )]
    pub media_url_ttl_seconds: u32,
    /// Signed browser upload ticket lifetime, in seconds.
    #[config(
        env = "UPLOAD_TICKET_TTL_SECONDS",
        default = 900,
        layer_attr(arg(long, global = true))
    )]
    pub upload_ticket_ttl_seconds: u32,
    /// Maximum total file bytes per upload batch or downloaded URL.
    #[config(
        env = "MAX_UPLOAD_BYTES",
        default = 536870912u64,
        layer_attr(arg(long, global = true))
    )]
    pub max_upload_bytes: u64,
    /// Concurrent workflow steps per worker process.
    #[config(
        env = "WORKER_CONCURRENCY",
        default = 2,
        layer_attr(arg(long, global = true))
    )]
    pub worker_concurrency: usize,
    /// Deadline per workflow step attempt, in seconds.
    #[config(
        env = "PROCESS_TIMEOUT_SECONDS",
        default = 300u64,
        layer_attr(arg(long, global = true))
    )]
    pub process_timeout_seconds: u64,
    #[config(
        env = "ALLOW_PRIVATE_SOURCE_URLS",
        default = false,
        layer_attr(arg(long, global = true))
    )]
    pub allow_private_source_urls: bool,
    /// Tracing log filter.
    #[config(
        env = "RUST_LOG",
        default = "booru_thingy=info",
        layer_attr(arg(long, global = true))
    )]
    pub rust_log: String,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_layer(ConfigLayer::default_values()).expect("valid built-in defaults")
    }
}

impl Config {
    pub fn load(cli: &Cli) -> anyhow::Result<Arc<Self>> {
        let cwd = std::env::current_dir().context("could not resolve working directory")?;
        let xdg = xdg_config_path(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        );
        Self::load_layers(cli, environment_layer()?, &cwd, xdg.as_deref())?.validate(cli.command)
    }

    fn load_layers(
        cli: &Cli,
        environment: ConfigLayer,
        cwd: &Path,
        xdg: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let mut layer = cli.overrides.clone().with_fallback(environment);
        if let Some(path) = &cli.config {
            layer = layer.with_fallback(file_layer(&cwd.join(path), true)?);
        }
        layer = layer.with_fallback(file_layer(&cwd.join("config.toml"), false)?);
        if let Some(path) = xdg {
            layer = layer.with_fallback(file_layer(path, false)?);
        }
        Self::from_layer(layer.with_fallback(ConfigLayer::default_values()))
            .map_err(|_| anyhow::anyhow!("could not resolve configuration"))
    }

    pub fn validate(self, command: Option<Command>) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(
            !self.database_url.is_empty(),
            "DATABASE_URL / database_url is required"
        );
        let database = url::Url::parse(&self.database_url)
            .map_err(|_| anyhow::anyhow!("DATABASE_URL / database_url must be a PostgreSQL URL"))?;
        anyhow::ensure!(
            matches!(database.scheme(), "postgres" | "postgresql") && database.host_str().is_some(),
            "DATABASE_URL / database_url must be a PostgreSQL URL"
        );
        tracing_subscriber::EnvFilter::try_new(&self.rust_log)
            .map_err(|_| anyhow::anyhow!("invalid RUST_LOG / rust_log"))?;
        if command == Some(Command::Migrate) {
            return Ok(Arc::new(self));
        }
        for (name, value) in [
            ("OWNER_API_KEY / owner_api_key", &self.owner_api_key),
            ("OWNER_LOGIN / owner_login", &self.owner_login),
            ("S3_BUCKET / s3_bucket", &self.s3_bucket),
            (
                "AWS_ACCESS_KEY_ID / aws_access_key_id",
                &self.aws_access_key_id,
            ),
            (
                "AWS_SECRET_ACCESS_KEY / aws_secret_access_key",
                &self.aws_secret_access_key,
            ),
        ] {
            anyhow::ensure!(!value.is_empty(), "{name} must not be empty");
        }
        for (name, value) in [
            ("BASE_URL / base_url", Some(&self.base_url)),
            ("S3_ENDPOINT / s3_endpoint", Some(&self.s3_endpoint)),
            (
                "S3_PUBLIC_ENDPOINT / s3_public_endpoint",
                self.s3_public_endpoint.as_ref(),
            ),
        ] {
            if let Some(value) = value {
                let url = url::Url::parse(value)
                    .map_err(|_| anyhow::anyhow!("{name} must be an HTTP(S) URL"))?;
                anyhow::ensure!(
                    matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                    "{name} must be an HTTP(S) URL"
                );
            }
        }
        let base = url::Url::parse(&self.base_url).unwrap();
        anyhow::ensure!(
            base.path() == "/"
                && base.query().is_none()
                && base.fragment().is_none()
                && base.username().is_empty()
                && base.password().is_none(),
            "BASE_URL must be an origin without a path or credentials"
        );
        anyhow::ensure!(
            (1..=604800).contains(&self.media_url_ttl_seconds),
            "media URL TTL must be 1..604800 seconds"
        );
        anyhow::ensure!(
            (1..=3600).contains(&self.upload_ticket_ttl_seconds),
            "upload ticket TTL must be 1..3600 seconds"
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
            self.process_timeout_seconds > 0,
            "processing timeout must be positive"
        );
        Ok(Arc::new(self))
    }
}

fn xdg_config_path(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let root = xdg
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".config"))
        })?;
    Some(root.join("booru-thingy/config.toml"))
}

fn file_layer(path: &Path, required: bool) -> anyhow::Result<ConfigLayer> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigLayer::empty());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not read configuration file {}", path.display()));
        }
    };
    toml::from_str(&text).map_err(|error: toml::de::Error| {
        let line = error.span().map(|span| {
            text[..span.start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1
        });
        anyhow::anyhow!(
            "invalid TOML configuration in {}{}: check syntax, key names, and value types",
            path.display(),
            line.map(|line| format!(" at line {line}"))
                .unwrap_or_default()
        )
    })
}

fn environment_layer() -> anyhow::Result<ConfigLayer> {
    let layer = ConfigLayer::from_env()
        .map_err(|_| anyhow::anyhow!("invalid configuration environment variable"))?;
    // Confique treats an unparseable empty value as absent. Reject that case
    // instead of silently falling back, while retaining valid empty strings.
    let values = serde_json::to_value(&layer)?;
    for field in Config::META.fields {
        if let confique::meta::FieldKind::Leaf {
            env: Some(name), ..
        } = field.kind
        {
            match std::env::var(name) {
                Ok(value) if value.is_empty() && values[field.name].is_null() => {
                    anyhow::bail!("invalid empty value for {name}")
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    anyhow::bail!("{name} must contain valid Unicode")
                }
                _ => {}
            }
        }
    }
    Ok(layer)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
