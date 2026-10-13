use crate::config::Config;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct Storage {
    client: Client,
    signer: Client,
    bucket: String,
    ttl: u32,
    upload_url: String,
    region: String,
    access_key: String,
    secret_key: String,
}
impl Storage {
    pub fn upload_origin(&self) -> String {
        url::Url::parse(&self.upload_url)
            .expect("validated upload URL")
            .origin()
            .ascii_serialization()
    }
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
                &config.aws_access_key_id,
                &config.aws_secret_access_key,
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
            ttl: config.media_url_ttl_seconds,
            upload_url: upload_url(config)?,
            region: config.s3_region.clone(),
            access_key: config.aws_access_key_id.clone(),
            secret_key: config.aws_secret_access_key.clone(),
        })
    }
    pub fn upload_ticket(&self, key: &str, size: i64, expires: DateTime<Utc>) -> Value {
        // The size condition limits accepted objects, not gateway resource usage:
        // SeaweedFS spools the form before checking it. Its proxy/gateway still
        // needs request-size limits, including multipart overhead.
        // Constrain ACL explicitly because permissive policy validators can accept
        // an injected public-read field. TODO: private is incompatible with AWS's
        // default ACL-disabled buckets; resolve this without dropping the constraint.
        let now = Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let credential = format!("{}/{scope}", self.access_key);
        let policy = STANDARD.encode(
            serde_json::to_vec(&json!({
                "expiration": expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "conditions": [
                    {"bucket": self.bucket}, {"key": key},
                    ["content-length-range", size, size],
                    {"Content-Type": "application/octet-stream"},
                    {"acl": "private"},
                    {"success_action_status": "204"},
                    {"x-amz-algorithm": "AWS4-HMAC-SHA256"},
                    {"x-amz-credential": credential}, {"x-amz-date": timestamp}
                ]
            }))
            .expect("serializable policy"),
        );
        let signing_key = hmac(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        );
        let signing_key = hmac(&signing_key, self.region.as_bytes());
        let signing_key = hmac(&signing_key, b"s3");
        let signing_key = hmac(&signing_key, b"aws4_request");
        let signature = hex::encode(hmac(&signing_key, policy.as_bytes()));
        json!({"url": self.upload_url, "expires_at": expires.to_rfc3339(), "fields": {
            "key": key, "Content-Type": "application/octet-stream",
            "acl": "private",
            "success_action_status": "204", "x-amz-algorithm": "AWS4-HMAC-SHA256",
            "x-amz-credential": credential, "x-amz-date": timestamp,
            "policy": policy, "x-amz-signature": signature
        }})
    }
    pub async fn validate_upload(
        &self,
        key: &str,
        etag: &str,
        expected: i64,
    ) -> anyhow::Result<()> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("uploaded object is not available"))?;
        anyhow::ensure!(
            head.content_length() == Some(expected),
            "uploaded object size does not match reservation"
        );
        anyhow::ensure!(
            head.e_tag() == Some(etag),
            "uploaded object changed; retry the transfer"
        );
        Ok(())
    }
    pub async fn put(&self, key: &str, path: &Path, content_type: &str) -> anyhow::Result<()> {
        // A precomputed header checksum keeps file uploads length-delimited. The
        // SDK's automatic streaming checksum uses aws-chunked trailers, which
        // Garage rejects. Keep integrity verification without provider branches.
        let mut file = tokio::fs::File::open(path).await?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; 65536];
        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .checksum_sha256(STANDARD.encode(digest.finalize()))
            .body(ByteStream::from_path(path).await?)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("object storage write failed"))?;
        Ok(())
    }
    pub async fn download(
        &self,
        key: &str,
        path: &Path,
        limit: u64,
        etag: Option<&str>,
    ) -> anyhow::Result<()> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_if_match(etag.map(str::to_owned))
            .send()
            .await
            .map_err(|_| {
                anyhow::anyhow!("object storage read failed: object unavailable or changed")
            })?;
        if let Some(etag) = etag {
            anyhow::ensure!(
                object.e_tag() == Some(etag),
                "uploaded object changed; upload file again"
            );
        }
        let mut body = object.body;
        let mut file = tokio::fs::File::create(path).await?;
        let mut size = 0_u64;
        while let Some(chunk) = body.try_next().await? {
            size += chunk.len() as u64;
            anyhow::ensure!(size <= limit, "input exceeds upload limit");
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
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

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn upload_url(config: &Config) -> anyhow::Result<String> {
    let mut url = url::Url::parse(
        config
            .s3_public_endpoint
            .as_deref()
            .unwrap_or(&config.s3_endpoint),
    )?;
    if config.s3_path_style {
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid storage endpoint"))?
            .pop_if_empty()
            .push(&config.s3_bucket);
    } else {
        let host = format!(
            "{}.{}",
            config.s3_bucket,
            url.host_str()
                .ok_or_else(|| anyhow::anyhow!("invalid storage endpoint"))?
        );
        url.set_host(Some(&host))?;
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Bytes, http::HeaderMap, routing::put};

    #[tokio::test]
    async fn file_put_uses_a_header_checksum_without_streaming_trailers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (send, mut receive) = tokio::sync::mpsc::channel(1);
        let routes = Router::new().route(
            "/test/object",
            put(move |headers: HeaderMap, bytes: Bytes| {
                let send = send.clone();
                async move {
                    send.send((headers, bytes)).await.unwrap();
                    axum::http::StatusCode::OK
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let storage = Storage::new(&Config {
            s3_endpoint: endpoint,
            s3_bucket: "test".into(),
            aws_access_key_id: "test-access".into(),
            aws_secret_access_key: "test-secret".into(),
            ..Config::default()
        })
        .unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), b"abc").await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            storage.put("object", file.path(), "application/octet-stream"),
        )
        .await
        .unwrap()
        .unwrap();
        let (headers, bytes) = receive.recv().await.unwrap();
        server.abort();
        assert_eq!(bytes.as_ref(), b"abc");
        assert_eq!(headers["content-length"], "3");
        assert_eq!(
            headers["x-amz-checksum-sha256"],
            "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="
        );
        assert!(!headers.contains_key("content-encoding"));
        assert!(!headers.contains_key("x-amz-trailer"));
        assert!(
            !headers["x-amz-content-sha256"]
                .to_str()
                .unwrap()
                .starts_with("STREAMING")
        );
    }
}
