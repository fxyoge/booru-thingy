use crate::{App, config::Config, router, storage::Storage, worker};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    body: Vec<u8>,
    content_type: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", content_type);
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    let body =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    (status, headers, body)
}
async fn get(router: &Router, path: &str, auth: &str) -> Value {
    let (status, _, body) = request(
        router,
        "GET",
        path,
        vec![],
        "application/x-www-form-urlencoded",
        &[("authorization", auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}
async fn generate(path: &Path, extension: &str) {
    let mut command = tokio::process::Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=c=green:s=64x48:d=0.3",
        "-threads",
        "1",
    ]);
    if !matches!(extension, "mp4" | "webm" | "gif" | "animated-webp") {
        command.args(["-frames:v", "1"]);
    }
    let output = command.arg(path).output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn multipart(files: &[(String, Vec<u8>)], tags: &str) -> Vec<u8> {
    let mut bytes = format!("--test-boundary\r\nContent-Disposition: form-data; name=\"upload[tag_string]\"\r\n\r\n{tags}\r\n").into_bytes();
    for (name, data) in files {
        bytes.extend_from_slice(format!("--test-boundary\r\nContent-Disposition: form-data; name=\"upload[files][]\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"--test-boundary--\r\n");
    bytes
}
async fn upload(router: &Router, auth: &str, files: &[(String, Vec<u8>)], tags: &str) -> Value {
    let (status, _, body) = request(
        router,
        "POST",
        "/uploads.json",
        multipart(files, tags),
        "multipart/form-data; boundary=test-boundary",
        &[("authorization", auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
async fn await_upload(router: &Router, auth: &str, id: i64) -> Value {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let upload = get(router, &format!("/uploads/{id}.json"), auth).await;
            if upload["status"] != "processing" {
                return upload;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("upload did not finish")
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires PostgreSQL, an S3 test endpoint, ffmpeg, and libvips"]
async fn durable_uploads_search_edits_favorites_and_sessions(pool: PgPool) {
    let bucket = format!("test-{}", uuid::Uuid::new_v4());
    let config = Config::parse_from([
        "test",
        "--database-url",
        "unused",
        "--owner-login",
        "owner",
        "--owner-api-key",
        "test-key",
        "--s3-endpoint",
        &std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:9000".into()),
        "--s3-bucket",
        &bucket,
        "--s3-access-key",
        &std::env::var("TEST_S3_ACCESS_KEY").unwrap_or_else(|_| "local-access-key".into()),
        "--s3-secret-key",
        &std::env::var("TEST_S3_SECRET_KEY").unwrap_or_else(|_| "local-secret-key".into()),
        "--worker-concurrency",
        "2",
        "--allow-private-source-urls",
        "true",
    ])
    .validate()
    .unwrap();
    let storage = Storage::new(&config).unwrap();
    storage.create_test_bucket().await.unwrap();
    let app = App {
        config,
        storage,
        db: pool.clone(),
    };
    let routes = router(app.clone());
    let auth = format!("Basic {}", STANDARD.encode("owner:test-key"));
    let (status, _, _) = request(&routes, "GET", "/posts.json", vec![], "", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        get(&routes, "/profile.json?login=owner&api_key=test-key", "").await["id"],
        1
    );
    let (status, headers, login) = request(
        &routes,
        "POST",
        "/login",
        br#"{"login":"owner","api_key":"test-key"}"#.to_vec(),
        "application/json",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = login["csrf_token"].as_str().unwrap();
    let (status, _, _) = request(
        &routes,
        "POST",
        "/favorites.json",
        b"post_id=1".to_vec(),
        "application/x-www-form-urlencoded",
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = request(
        &routes,
        "POST",
        "/login",
        br#"{"login":"owner","api_key":"test-key"}"#.to_vec(),
        "application/json",
        &[("origin", "https://another-origin.invalid")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let directory = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for ext in ["jpg", "png", "gif", "webp", "mp4", "webm"] {
        let path = directory.path().join(format!("fixture.{ext}"));
        generate(&path, ext).await;
        files.push((
            format!("fixture.{ext}"),
            tokio::fs::read(path).await.unwrap(),
        ));
    }
    files.push(files[1].clone()); // Same input processed by concurrent workers.
    let path = directory.path().join("animated.webp");
    let output = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=64x48:d=0.3",
            "-threads",
            "1",
            "-c:v",
            "libwebp_anim",
        ])
        .arg(&path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    files.push(("animated.webp".into(), tokio::fs::read(path).await.unwrap()));
    let accepted = upload(&routes, &auth, &files, "cat green").await;
    assert_eq!(accepted["status"], "processing");
    let upload_id = accepted["id"].as_i64().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        8
    );
    // Simulate a process disappearing after claiming a durable input.
    sqlx::query("UPDATE jobs SET lease_token='dead-worker',lease_until=now()-interval '1 second',attempts=1 WHERE id=(SELECT min(id) FROM jobs)").execute(&pool).await.unwrap();
    let stop = CancellationToken::new();
    let workers = tokio::spawn(worker::run(app.clone(), stop.clone()));
    let completed = await_upload(&routes, &auth, upload_id).await;
    assert_eq!(completed["status"], "completed", "{completed}");
    let animated = completed["upload_media_assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["filename"] == "animated.webp")
        .unwrap();
    assert!(animated["media_asset"]["duration"].as_f64().unwrap() > 0.0);
    assert_eq!(
        animated["media_asset"]["variants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["type"] == "sample")
            .unwrap()["file_ext"],
        "webp"
    );
    let count = get(&routes, "/counts/posts.json?limit=1", &auth).await;
    assert_eq!(count["counts"]["posts"], 7);
    assert_eq!(
        completed["upload_media_assets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["duplicate"] == true)
            .count(),
        1
    );
    let posts = get(&routes, "/posts.json?tags=cat%20-green", &auth).await;
    assert_eq!(posts, json!([]));
    let posts = get(&routes, "/posts.json?tags=cat&limit=1", &auth).await;
    assert_eq!(posts.as_array().unwrap().len(), 1);
    let post_id = posts[0]["id"].as_i64().unwrap();
    let post = &posts[0];
    assert_eq!(post["tag_count_general"], 2);
    let media_url = post["preview_file_url"].as_str().unwrap();
    assert!(reqwest::get(media_url).await.unwrap().status().is_success());
    let variants = post["media_asset"]["variants"].as_array().unwrap();
    assert_eq!(variants.len(), 6);
    assert!(
        variants
            .iter()
            .all(|v| v["width"].as_i64().unwrap() > 0 && v["height"].as_i64().unwrap() > 0)
    );
    let query = format!(
        "/posts/{post_id}.json?post%5Btag_string%5D=-green%20bird%20rating%3Aq&post%5Bold_tag_string%5D="
    );
    let (status, _, edited) = request(
        &routes,
        "PUT",
        &query,
        vec![],
        "application/x-www-form-urlencoded",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["tag_string"], "bird cat");
    assert_eq!(edited["rating"], "q");
    assert_eq!(
        get(&routes, "/counts/posts.json?tags=rating%3Aq", &auth).await["counts"]["posts"],
        1
    );
    let tags = get(&routes, "/tags.json?search[name_comma]=green,bird", &auth).await;
    assert_eq!(
        tags.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "green")
            .unwrap()["post_count"],
        6
    );
    for _ in 0..2 {
        let (status, _, _) = request(
            &routes,
            "POST",
            "/favorites.json",
            format!("post_id={post_id}").into_bytes(),
            "application/x-www-form-urlencoded",
            &[("cookie", &cookie), ("x-csrf-token", csrf)],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(
        get(&routes, "/favorites.json", &auth)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        get(&routes, "/counts/posts.json?tags=fav%3Aowner", &auth).await["counts"]["posts"],
        1
    );
    let duplicate = upload(
        &routes,
        &auth,
        &[files
            .iter()
            .find(|f| {
                completed["upload_media_assets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["filename"] == f.0 && i["post_id"] == post_id)
            })
            .unwrap()
            .clone()],
        "overwrite_attempt",
    )
    .await;
    let duplicate = await_upload(&routes, &auth, duplicate["id"].as_i64().unwrap()).await;
    assert_eq!(duplicate["status"], "completed");
    assert_eq!(
        get(&routes, &format!("/posts/{post_id}.json"), &auth).await["tag_string"],
        "bird cat"
    );
    let invalid = upload(
        &routes,
        &auth,
        &[("invalid.png".into(), b"not an image".to_vec())],
        "tagme",
    )
    .await;
    sqlx::query("UPDATE jobs SET attempts=2 WHERE upload_item_id IN (SELECT id FROM upload_items WHERE upload_id=$1)").bind(invalid["id"].as_i64().unwrap()).execute(&pool).await.unwrap();
    let failed = await_upload(&routes, &auth, invalid["id"].as_i64().unwrap()).await;
    assert_eq!(failed["status"], "error");
    assert!(failed["error"].as_str().unwrap().contains("unsupported"));

    assert_eq!(
        get(&routes, "/users/1/uploads.json?search[status]=error", &auth)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        get(
            &routes,
            "/users/1/uploads.json?search[is_posted]=true",
            &auth
        )
        .await
        .as_array()
        .unwrap()
        .len(),
        2
    );
    let (status, _, _) = request(
        &routes,
        "PUT",
        &format!("/posts/{post_id}.json"),
        br#"{"post":{"tag_string":"should_rollback","parent_id":999999}}"#.to_vec(),
        "application/json",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get(&routes, &format!("/posts/{post_id}.json"), &auth).await["tag_string"],
        "bird cat"
    );

    // Exercise real URL requests, redirects, and interrupted-worker recovery.
    let slow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let source_routes = Router::new()
        .route(
            "/image",
            axum::routing::get({
                let png = files[1].1.clone();
                move || {
                    let png = png.clone();
                    async move { ([("content-type", "image/png")], png) }
                }
            }),
        )
        .route(
            "/redirect",
            axum::routing::get(|| async { axum::response::Redirect::temporary("/image") }),
        )
        .route(
            "/slow",
            axum::routing::get({
                let slow = slow.clone();
                let started = started.clone();
                let png = files[1].1.clone();
                move || {
                    let slow = slow.clone();
                    let started = started.clone();
                    let png = png.clone();
                    async move {
                        started.notify_one();
                        if slow.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                        ([("content-type", "image/png")], png)
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_origin = format!("http://{}", listener.local_addr().unwrap());
    let source_server = tokio::spawn(async move {
        axum::serve(listener, source_routes).await.unwrap();
    });
    let mut restricted = app.config.as_ref().clone();
    restricted.allow_private_source_urls = false;
    let restricted = App {
        config: std::sync::Arc::new(restricted),
        ..app.clone()
    };
    assert!(
        worker::fetch(
            &restricted,
            &format!("{source_origin}/image"),
            &directory.path().join("restricted")
        )
        .await
        .is_err()
    );
    let (status, _, url_upload) = request(
        &routes,
        "POST",
        "/uploads.json",
        json!({"upload":{"source":format!("{source_origin}/redirect")}})
            .to_string()
            .into_bytes(),
        "application/json",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url_upload = await_upload(&routes, &auth, url_upload["id"].as_i64().unwrap()).await;
    assert_eq!(url_upload["status"], "completed");
    assert_eq!(url_upload["upload_media_assets"][0]["duplicate"], true);
    let (status, _, interrupted) = request(
        &routes,
        "POST",
        "/uploads.json",
        format!(
            "upload%5Bsource%5D={}",
            url::form_urlencoded::byte_serialize(format!("{source_origin}/slow").as_bytes())
                .collect::<String>()
        )
        .into_bytes(),
        "application/x-www-form-urlencoded",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(2), workers)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let interrupted_id = interrupted["id"].as_i64().unwrap();
    assert_eq!(
        get(&routes, &format!("/uploads/{interrupted_id}.json"), &auth).await["status"],
        "processing"
    );
    slow.store(false, std::sync::atomic::Ordering::SeqCst);
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE upload_item_id IN (SELECT id FROM upload_items WHERE upload_id=$1)").bind(interrupted_id).execute(&pool).await.unwrap();
    let stop = CancellationToken::new();
    let resumed = tokio::spawn(worker::run(app.clone(), stop.clone()));
    assert_eq!(
        await_upload(&routes, &auth, interrupted_id).await["status"],
        "completed"
    );
    assert_eq!(
        get(&routes, "/counts/posts.json", &auth).await["counts"]["posts"],
        7
    );
    stop.cancel();
    resumed.await.unwrap().unwrap();
    source_server.abort();
}
