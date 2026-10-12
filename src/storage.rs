use crate::config::Config;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use std::{path::Path, time::Duration};
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct Storage {
    client: Client,
    signer: Client,
    bucket: String,
    ttl: u32,
}
impl Storage {
    #[cfg(test)]
    pub async fn create_test_bucket(&self) -> anyhow::Result<()> {
        self.client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await?;
        Ok(())
    }
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let endpoint = url::Url::parse(&config.s3_endpoint)?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https"),
            "S3_ENDPOINT must be HTTP(S)"
        );
        let sdk = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.s3_region.clone()))
            .credentials_provider(Credentials::new(
                &config.s3_access_key,
                &config.s3_secret_key,
                None,
                None,
                "configuration",
            ))
            .endpoint_url(&config.s3_endpoint)
            .force_path_style(config.s3_path_style)
            .build();
        let client = Client::from_conf(sdk.clone());
        let signer = if let Some(endpoint) = &config.s3_public_endpoint {
            let parsed = url::Url::parse(endpoint)?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https"),
                "S3_PUBLIC_ENDPOINT must be HTTP(S)"
            );
            Client::from_conf(sdk.to_builder().endpoint_url(endpoint).build())
        } else {
            client.clone()
        };
        Ok(Self {
            client,
            signer,
            bucket: config.s3_bucket.clone(),
            ttl: config.media_url_ttl,
        })
    }
    pub async fn put(&self, key: &str, path: &Path, content_type: &str) -> anyhow::Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from_path(path).await?)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("object storage write failed"))?;
        Ok(())
    }
    pub async fn download(&self, key: &str, path: &Path, limit: u64) -> anyhow::Result<()> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("object storage read failed"))?;
        let mut body = object.body;
        let mut file = tokio::fs::File::create(path).await?;
        let mut size = 0_u64;
        while let Some(chunk) = body.try_next().await? {
            size += chunk.len() as u64;
            anyhow::ensure!(size <= limit, "input exceeds upload limit");
            file.write_all(&chunk).await?;
        }
        Ok(())
    }
    pub async fn url(&self, key: &str) -> anyhow::Result<String> {
        let request = self
            .signer
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(Duration::from_secs(
                self.ttl as u64,
            ))?)
            .await
            .map_err(|_| anyhow::anyhow!("media URL signing failed"))?;
        Ok(request.uri().to_owned())
    }
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("object deletion failed"))?;
        Ok(())
    }
    pub async fn ready(&self) -> anyhow::Result<()> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("bucket unavailable"))?;
        Ok(())
    }
}
