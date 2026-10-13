//! One idempotent media operation per queue task. Retries repeat media work.
use crate::{
    App, api,
    db::{generated as queries, optional},
    media,
};
use futures_util::StreamExt;
use std::{net::IpAddr, path::Path, time::Duration};
use tokio::io::AsyncWriteExt;

pub async fn process(app: &App, item_id: i64, task_id: &str) -> anyhow::Result<()> {
    let row = queries::get_processing_item(&app.db, item_id).await?;
    if row.post_id.is_some() || row.task_id.as_deref() != Some(task_id) {
        return Ok(());
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("input");
    if let Some(key) = &row.staging_key {
        app.storage
            .download(key, &path, app.config.max_upload_bytes)
            .await?;
    } else {
        fetch(app, &row.source_url, &path).await?;
    }
    let metadata = media::inspect(&path).await?;
    let existing = optional(queries::get_asset_by_hash(&app.db, &metadata.sha256).await)?;
    let variants = if existing.is_some_and(|a| a.status == "active") {
        Vec::new()
    } else {
        let key = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
        app.storage.put(&key, &path, &metadata.content_type).await?;
        media::variants(app, &metadata, &path, directory.path()).await?
    };
    publish(app, item_id, task_id, &metadata, variants).await
}

async fn publish(
    app: &App,
    item_id: i64,
    task_id: &str,
    metadata: &media::Metadata,
    variants: Vec<media::Variant>,
) -> anyhow::Result<()> {
    let mut tx = app.db.begin().await?;
    let row = queries::lock_processing_item(&mut tx, item_id).await?;
    // Publication is immutable. Obsolete deliveries may write hash-addressed
    // objects but cannot publish after a manual retry replaces the task identity.
    if row.post_id.is_some() || row.task_id.as_deref() != Some(task_id) {
        return Ok(());
    }
    let asset = optional(queries::get_asset_by_hash(&mut tx, &metadata.sha256).await)?;
    let asset_id = if let Some(asset) = asset.as_ref().filter(|r| r.status == "active") {
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
            id: item_id,
            media_asset_id: Some(asset_id),
            post_id: Some(post_id),
            duplicate,
        },
    )
    .await?;
    queries::touch_upload(&mut tx, row.upload_id).await?;
    tx.commit().await?;
    tracing::info!(item_id, post_id, duplicate, "upload published");
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
    // Failed inputs remain available for manual retries. Only publication makes
    // the accepted snapshot redundant with the hash-addressed original.
    for row in queries::list_staging_cleanup_candidates(&app.db).await? {
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

    async fn item(pool: &sqlx::PgPool) -> (i64, String) {
        let mut tx = pool.begin().await.unwrap();
        let upload_id = queries::create_upload(&mut tx).await.unwrap().id;
        let item_id = queries::create_source_item(
            &mut tx,
            queries::CreateSourceItemParams {
                upload_id,
                source_url: "https://example.invalid/image.png",
                filename: "image.png",
            },
        )
        .await
        .unwrap()
        .id;
        crate::queue::enqueue(&mut tx, item_id).await.unwrap();
        tx.commit().await.unwrap();
        let task_id = queries::get_processing_item(pool, item_id)
            .await
            .unwrap()
            .task_id
            .unwrap();
        (item_id, task_id)
    }

    fn metadata() -> media::Metadata {
        media::Metadata {
            sha256: "same-hash".into(),
            md5: "same-md5".into(),
            ext: "png".into(),
            content_type: "image/png".into(),
            size: 100,
            width: 64,
            height: 48,
            duration: None,
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    #[ignore = "requires PostgreSQL"]
    async fn publication_converges_and_survives_unacknowledged_replay(pool: sqlx::PgPool) {
        let app = crate::queue::tests::app(pool.clone());
        let (one, a) = item(&pool).await;
        let (two, b) = item(&pool).await;
        let metadata = metadata();
        let (left, right) = tokio::join!(
            publish(&app, one, &a, &metadata, Vec::new()),
            publish(&app, two, &b, &metadata, Vec::new())
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
        // Queue tasks are still Pending: the committed business outcome wins.
        assert_eq!(
            queries::get_upload_item_state(&pool, one)
                .await
                .unwrap()
                .status,
            "completed"
        );
        process(&app, one, &a).await.unwrap();
        process(&app, two, &b).await.unwrap();
        publish(&app, one, &a, &metadata, Vec::new()).await.unwrap();
        let replayed: Vec<(i64, bool)> =
            sqlx::query_as("SELECT post_id,duplicate FROM upload_items ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, replayed);
        sqlx::query("DELETE FROM apalis.jobs")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            queries::get_upload_item_state(&pool, one)
                .await
                .unwrap()
                .status,
            "completed"
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
    async fn obsolete_tasks_cannot_publish_or_fetch_input(pool: sqlx::PgPool) {
        let app = crate::queue::tests::app(pool.clone());
        let (item_id, old_task) = item(&pool).await;
        let mut tx = pool.begin().await.unwrap();
        crate::queue::enqueue(&mut tx, item_id).await.unwrap();
        tx.commit().await.unwrap();
        process(&app, item_id, &old_task).await.unwrap();
        publish(&app, item_id, &old_task, &metadata(), Vec::new())
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM posts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            queries::get_upload_item_state(&pool, item_id)
                .await
                .unwrap()
                .status,
            "pending"
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
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
