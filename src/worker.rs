use crate::{App, api, media};
use futures_util::StreamExt;
use sqlx::Row;
use std::{net::IpAddr, path::Path, time::Duration};
use tokio::{io::AsyncWriteExt, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub async fn run(app: App, stop: CancellationToken) -> anyhow::Result<()> {
    let mut tasks = JoinSet::new();
    let mut maintenance = tokio::time::interval(Duration::from_secs(3600));
    while !stop.is_cancelled() {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = maintenance.tick() => { if let Err(error) = cleanup(&app).await { tracing::warn!(%error,"maintenance failed"); } },
            result = tasks.join_next(), if !tasks.is_empty() => { if let Some(Err(error)) = result { tracing::error!(%error,"worker task failed"); } },
            _ = tokio::time::sleep(Duration::from_millis(500)), if tasks.len() < app.config.worker_concurrency => {
                let token = uuid::Uuid::new_v4().to_string();
                let row = sqlx::query("UPDATE jobs SET lease_until=now()+interval '60 seconds',lease_token=$1,attempts=attempts+1 WHERE id=(SELECT id FROM jobs WHERE completed_at IS NULL AND available_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,upload_item_id,attempts").bind(&token).fetch_optional(&app.db).await;
                match row {
                    Ok(Some(row)) => { let app = app.clone(); tasks.spawn(async move { execute(app,row.get("id"),row.get("upload_item_id"),row.get("attempts"),token).await; }); },
                    Ok(None) => {},
                    Err(error) => { tracing::error!(%error,"job claim failed"); tokio::time::sleep(Duration::from_secs(3)).await; }
                }
            }
        }
    }
    // Cancel scratch work promptly. Leases expire and another worker resumes it.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
async fn execute(app: App, job_id: i64, item_id: i64, attempts: i32, token: String) {
    let heartbeat_stop = CancellationToken::new();
    let _heartbeat_guard = heartbeat_stop.clone().drop_guard();
    let heartbeat = {
        let app = app.clone();
        let token = token.clone();
        let stop = heartbeat_stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(15)) => {
                        if sqlx::query("UPDATE jobs SET lease_until=now()+interval '60 seconds' WHERE id=$1 AND lease_token=$2 AND completed_at IS NULL").bind(job_id).bind(&token).execute(&app.db).await.map(|r| r.rows_affected()).unwrap_or(0) == 0 { break; }
                    }
                }
            }
        })
    };
    let result = media::with_timeout(
        app.config.process_timeout,
        process(&app, job_id, item_id, &token),
    )
    .await;
    heartbeat_stop.cancel();
    let _ = heartbeat.await;
    if let Err(error) = result {
        let message = safe_error(&error);
        tracing::warn!(job_id,item_id,attempts,error=%message,"media processing failed");
        let result: anyhow::Result<()> = async {
            let mut tx = app.db.begin().await?;
            let changed = sqlx::query("UPDATE jobs SET last_error=$3,lease_until=NULL,lease_token=NULL,available_at=now()+interval '10 seconds' * $4,completed_at=CASE WHEN $4 >= 3 THEN now() ELSE NULL END WHERE id=$1 AND lease_token=$2 AND completed_at IS NULL RETURNING id")
                .bind(job_id).bind(&token).bind(&message).bind(attempts).fetch_optional(&mut *tx).await?;
            if changed.is_some() { sqlx::query("UPDATE upload_items SET status=$2,error=$3,updated_at=now() WHERE id=$1").bind(item_id).bind(if attempts >= 3 { "error" } else { "pending" }).bind(&message).execute(&mut *tx).await?; }
            tx.commit().await?; Ok(())
        }.await;
        if let Err(error) = result {
            tracing::error!(%error,job_id,"could not record job outcome");
        }
    }
}
fn safe_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    // URL fetch errors are intentionally generated without including the URL.
    if message.contains("http://") || message.contains("https://") {
        "media operation failed".into()
    } else {
        message.chars().take(300).collect()
    }
}
async fn process(app: &App, job_id: i64, item_id: i64, token: &str) -> anyhow::Result<()> {
    let row = sqlx::query("SELECT i.*,u.source,u.tag_string,u.rating FROM upload_items i JOIN uploads u ON u.id=i.upload_id WHERE i.id=$1").bind(item_id).fetch_one(&app.db).await?;
    sqlx::query(
        "UPDATE upload_items SET status='processing',error=NULL,updated_at=now() WHERE id=$1",
    )
    .bind(item_id)
    .execute(&app.db)
    .await?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("input");
    if let Some(key) = row.get::<Option<String>, _>("staging_key") {
        app.storage
            .download(&key, &path, app.config.max_upload_bytes)
            .await?;
    } else {
        fetch(app, &row.get::<String, _>("source_url"), &path).await?;
    }
    let metadata = media::inspect(&path).await?;
    let mut tx = app.db.begin().await?;
    // Hash-scoped transaction lock makes concurrent deduplication deterministic.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&metadata.sha256)
        .execute(&mut *tx)
        .await?;
    let asset = sqlx::query("SELECT id,status FROM media_assets WHERE sha256=$1")
        .bind(&metadata.sha256)
        .fetch_optional(&mut *tx)
        .await?;
    let asset_id: i64 = if let Some(asset) = asset
        .as_ref()
        .filter(|r| r.get::<String, _>("status") == "active")
    {
        asset.get("id")
    } else {
        let variants = media::variants(app, &metadata, &path, directory.path()).await?;
        let original = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
        let id: i64 = sqlx::query_scalar("INSERT INTO media_assets(sha256,md5,file_ext,content_type,file_size,image_width,image_height,duration,original_key,status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'active') ON CONFLICT(sha256) DO UPDATE SET status='active',updated_at=now() RETURNING id")
            .bind(&metadata.sha256).bind(&metadata.md5).bind(metadata.ext).bind(metadata.content_type).bind(metadata.size).bind(metadata.width).bind(metadata.height).bind(metadata.duration).bind(original).fetch_one(&mut *tx).await?;
        for variant in variants {
            sqlx::query("INSERT INTO media_variants(media_asset_id,kind,object_key,file_ext,content_type,width,height,file_size) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(media_asset_id,kind) DO UPDATE SET object_key=EXCLUDED.object_key,width=EXCLUDED.width,height=EXCLUDED.height,file_size=EXCLUDED.file_size")
            .bind(id).bind(variant.kind).bind(variant.key).bind(variant.ext).bind(variant.content_type).bind(variant.width).bind(variant.height).bind(variant.size).execute(&mut *tx).await?;
        }
        id
    };
    let tags = api::normalize_tags(&row.get::<String, _>("tag_string"))
        .map_err(|e| anyhow::anyhow!(e.1))?;
    let (post_id, duplicate) = api::publish(
        &mut tx,
        asset_id,
        &row.get::<String, _>("source"),
        &tags,
        &row.get::<String, _>("rating"),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    let changed = sqlx::query("UPDATE jobs SET completed_at=now(),lease_until=NULL,last_error=NULL WHERE id=$1 AND lease_token=$2 AND completed_at IS NULL").bind(job_id).bind(token).execute(&mut *tx).await?;
    anyhow::ensure!(changed.rows_affected() == 1, "job lease lost");
    sqlx::query("UPDATE upload_items SET status='completed',media_asset_id=$2,post_id=$3,duplicate=$4,error=NULL,updated_at=now() WHERE id=$1").bind(item_id).bind(asset_id).bind(post_id).bind(duplicate).execute(&mut *tx).await?;
    sqlx::query("UPDATE uploads SET updated_at=now() WHERE id=$1")
        .bind(row.get::<i64, _>("upload_id"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(job_id, item_id, post_id, duplicate, "upload published");
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
            .timeout(Duration::from_secs(app.config.process_timeout))
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
async fn cleanup(app: &App) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM web_sessions WHERE expires_at<now()")
        .execute(&app.db)
        .await?;
    sqlx::query("UPDATE upload_items SET status='error',error='Upload interrupted',updated_at=now() WHERE status='receiving' AND updated_at<now()-interval '24 hours'").execute(&app.db).await?;
    let rows = sqlx::query("SELECT id,staging_key FROM upload_items WHERE staging_key IS NOT NULL AND (status='completed' OR (status='error' AND updated_at<now()-interval '7 days')) LIMIT 100").fetch_all(&app.db).await?;
    for row in rows {
        let mut tx = app.db.begin().await?;
        let key = sqlx::query_scalar::<_,Option<String>>("SELECT staging_key FROM upload_items WHERE id=$1 AND (status='completed' OR (status='error' AND updated_at<now()-interval '7 days')) FOR UPDATE").bind(row.get::<i64,_>("id")).fetch_optional(&mut *tx).await?.flatten();
        if let Some(key) = key {
            app.storage.delete(&key).await?;
            sqlx::query("UPDATE upload_items SET staging_key=NULL WHERE id=$1")
                .bind(row.get::<i64, _>("id"))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
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
