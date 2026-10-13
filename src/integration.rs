use crate::{
    App, config::Config, db::generated as queries, queue, router, storage::Storage, worker,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
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
async fn upload(router: &Router, auth: &str, files: &[(String, Vec<u8>)], tags: &str) -> Value {
    let (status, _, body) = request(
        router,
        "POST",
        "/uploads/direct",
        serde_json::to_vec(&json!({"tags": tags, "files": files.iter().map(|(name, bytes)| json!({"filename":name,"size":bytes.len()})).collect::<Vec<_>>()})).unwrap(),
        "application/json",
        &[("authorization", auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (item, (name, data)) in body["upload_media_assets"]
        .as_array()
        .unwrap()
        .iter()
        .zip(files)
    {
        let path = format!("/uploads/{}/items/{}", body["id"], item["id"]);
        let (status, _, ticket) = request(
            router,
            "POST",
            &format!("{path}/ticket"),
            vec![],
            "",
            &[("authorization", auth)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        let mut multipart = Vec::new();
        for (key, value) in ticket["fields"].as_object().unwrap() {
            multipart.extend_from_slice(format!("--test-boundary\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{}\r\n",value.as_str().unwrap()).as_bytes());
        }
        multipart.extend_from_slice(format!("--test-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        multipart.extend_from_slice(data);
        multipart.extend_from_slice(b"\r\n--test-boundary--\r\n");
        let response = reqwest::Client::new()
            .post(ticket["url"].as_str().unwrap())
            .header(
                "content-type",
                "multipart/form-data; boundary=test-boundary",
            )
            .body(multipart)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        let (status, _, result) = request(
            router,
            "POST",
            &format!("{path}/complete"),
            vec![],
            "",
            &[("authorization", auth)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
    }
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

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "requires PostgreSQL"]
async fn url_upload_creation_is_atomic(pool: PgPool) {
    let config = std::sync::Arc::new(Config {
        owner_api_key: "test-key".into(),
        s3_endpoint: "http://127.0.0.1:9000".into(),
        s3_bucket: "test".into(),
        aws_access_key_id: "test-access".into(),
        aws_secret_access_key: "test-secret".into(),
        ..Config::default()
    });
    let routes = router(App {
        storage: Storage::new(&config).unwrap(),
        config,
        db: pool.clone(),
    });
    let auth = format!("Basic {}", STANDARD.encode("owner:test-key"));
    for fields in [
        json!({"source":""}),
        json!({"source":"file:///etc/passwd"}),
        json!({"source":"https://user:password@example.com/image.png"}),
        json!({"source":"https://example.com/image.png","rating":"invalid"}),
        json!({"source":"https://example.com/image.png","tag_string":"invalid:tag"}),
    ] {
        let (status, _, body) = request(
            &routes,
            "POST",
            "/uploads.json",
            json!({"upload":fields}).to_string().into_bytes(),
            "application/json",
            &[("authorization", &auth)],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, _, _) = request(
        &routes,
        "POST",
        "/uploads.json",
        vec![],
        "multipart/form-data; boundary=test",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for (content_type, body, tags, rating, referer) in [
        (
            "application/json",
            json!({"upload":{"source":"https://example.com/image.png"}})
                .to_string(),
            "tagme",
            "g",
            "",
        ),
        (
            "application/x-www-form-urlencoded",
            "upload[source]=https%3A%2F%2Fexample.com%2Fimage.png&upload[tag_string]=DOG+cat+dog&upload[rating]=q&upload[referer_url]=https%3A%2F%2Fexample.com".into(),
            "cat dog",
            "q",
            "https://example.com",
        ),
    ] {
        let (status, _, upload) = request(
            &routes,
            "POST",
            "/uploads.json",
            body.into_bytes(),
            content_type,
            &[("authorization", &auth)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{upload}");
        assert_eq!(upload["source"], "https://example.com/image.png");
        assert_eq!(upload["tag_string"], tags);
        assert_eq!(upload["rating"], rating);
        assert_eq!(upload["referer_url"], referer);
        let items = upload["upload_media_assets"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["filename"], "image.png");
        assert_eq!(items[0]["status"], "pending");
    }

    // Fail at the final insertion, after creating the upload and its source item.
    sqlx::query("ALTER TABLE apalis.jobs ADD CONSTRAINT reject_enqueue CHECK (false) NOT VALID")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _, _) = request(
        &routes,
        "POST",
        "/uploads.json",
        json!({"upload":{"source":"https://example.com/rollback.png"}})
            .to_string()
            .into_bytes(),
        "application/json",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM uploads), (SELECT count(*) FROM upload_items), (SELECT count(*) FROM apalis.jobs)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (2, 2, 2));
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "requires PostgreSQL"]
async fn upload_lists_batch_reads_and_preserve_response_semantics(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    sqlx::raw_sql(
        "INSERT INTO uploads(id,source) OVERRIDING SYSTEM VALUE SELECT id,'https://example.com/' || id FROM generate_series(1,5) id;
        INSERT INTO media_assets(id,sha256,md5,file_ext,content_type,file_size,image_width,image_height,original_key,status)
        VALUES(1,'one','one','png','image/png',100,64,48,'media/one/original.png','active'),
              (2,'two','two','jpg','image/jpeg',200,80,60,'media/two/original.jpg','active');
        INSERT INTO media_variants(media_asset_id,kind,object_key,file_ext,content_type,width,height,file_size)
        VALUES(1,'original','media/one/original.png','png','image/png',64,48,100),
              (1,'180x180','media/one/preview.jpg','jpg','image/jpeg',64,48,50),
              (2,'original','media/two/original.jpg','jpg','image/jpeg',80,60,200);
        INSERT INTO posts(id,media_asset_id) VALUES(1,1),(2,2);
        INSERT INTO favorites(post_id) VALUES(1);
        INSERT INTO apalis.jobs(id,job_type,job,status,attempts,max_attempts,last_result)
        VALUES('failed-22','uploads','0','Failed',3,3,jsonb_build_object('Err','invalid media')),
              ('pending-32','uploads','0','Failed',1,3,jsonb_build_object('Err','retrying')),
              ('failed-41','uploads','0','Failed',3,3,jsonb_build_object('Err','download failed'));
        INSERT INTO upload_items(id,upload_id,filename,task_id,media_asset_id,post_id,duplicate)
        OVERRIDING SYSTEM VALUE
        VALUES(11,1,'first.png',NULL,1,1,false),
              (12,1,'duplicate.png',NULL,1,1,true),
              (21,2,'second.jpg',NULL,2,2,false),
              (22,2,'broken.png','failed-22',NULL,NULL,false),
              (31,3,'receiving.png',NULL,NULL,NULL,false),
              (32,3,'pending.png','pending-32',1,NULL,false),
              (41,4,'failed.png','failed-41',NULL,NULL,false);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    // Every standalone pool query acquires a connection. Only API reads use this pool.
    let app_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .before_acquire({
            let reads = reads.clone();
            move |_, _| {
                reads.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(true) })
            }
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let config = Arc::new(Config {
        owner_api_key: "test-key".into(),
        s3_endpoint: "http://127.0.0.1:9000".into(),
        s3_bucket: "test".into(),
        aws_access_key_id: "test-access".into(),
        aws_secret_access_key: "test-secret".into(),
        ..Config::default()
    });
    let routes = router(App {
        storage: Storage::new(&config).unwrap(),
        config,
        db: app_pool.clone(),
    });
    let auth = format!("Basic {}", STANDARD.encode("owner:test-key"));
    for (path, ids, expected_reads) in [
        ("/uploads.json", vec![5, 4, 3, 2, 1], 4),
        ("/users/1/uploads.json", vec![5, 4, 3, 2, 1], 4),
        (
            "/uploads.json?search[order]=id_asc&limit=2&page=2",
            vec![3, 4],
            4,
        ),
        ("/uploads.json?search[status]=completed", vec![1], 4),
        ("/uploads.json?search[status]=error", vec![5, 4, 2], 4),
        ("/uploads.json?search[status]=processing", vec![3], 4),
        ("/uploads.json?search[is_posted]=true", vec![2, 1], 4),
        ("/uploads.json?search[is_posted]=false", vec![5, 4, 3], 4),
        ("/uploads.json?search[ai_tags_match]=unsupported", vec![], 0),
        ("/uploads.json?page=100", vec![], 1),
    ] {
        reads.store(0, Ordering::SeqCst);
        let uploads = get(&routes, path, &auth).await;
        assert_eq!(reads.load(Ordering::SeqCst), expected_reads, "{path}");
        let actual: Vec<_> = uploads
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["id"].as_i64().unwrap())
            .collect();
        assert_eq!(actual, ids, "{path}");
    }
    let uploads = get(&routes, "/uploads.json", &auth).await;
    assert_eq!(uploads[0]["status"], "error"); // Empty upload.
    assert!(uploads[0]["error"].is_null());
    assert_eq!(uploads[1]["error"], "download failed");
    assert_eq!(uploads[2]["status"], "processing");
    assert!(uploads[2]["error"].is_null()); // Retryable queue errors are not terminal upload errors.
    assert_eq!(uploads[3]["status"], "error"); // Mixed success/failure.
    assert_eq!(uploads[3]["media_asset_count"], 1);
    let completed = &uploads[4];
    assert_eq!(completed["status"], "completed");
    assert_eq!(completed["media_asset_count"], 2); // Preserve per-item duplicates.
    assert_eq!(completed["posts"], json!([{"id":1},{"id":1}]));
    assert_eq!(completed["upload_media_assets"][1]["duplicate"], true);
    assert_eq!(
        completed["upload_media_assets"][0]["page_url"],
        "https://example.com/1"
    );
    let shared = &completed["media_assets"][0];
    assert_eq!(shared["post"], json!({"id":1}));
    assert_eq!(shared["variants"].as_array().unwrap().len(), 2);
    assert!(shared.get("original_key").is_none());
    assert!(shared.get("sha256").is_none());
    for variant in shared["variants"].as_array().unwrap() {
        assert!(variant.get("object_key").is_none());
        let url = url::Url::parse(variant["url"].as_str().unwrap()).unwrap();
        assert!(url.query_pairs().any(|(key, _)| key == "X-Amz-Signature"));
    }
    // Attaching a post to one item must not mutate a shared asset in another upload.
    assert!(uploads[2]["media_assets"][0].get("post").is_none());
    for upload in uploads.as_array().unwrap() {
        assert_eq!(upload["uploader"]["post_upload_count"], 2);
        assert_eq!(upload["uploader"]["favorite_count"], 1);
    }
    for (path, ids, expected_reads) in [
        (
            "/upload_media_assets.json",
            vec![41, 31, 32, 21, 22, 11, 12],
            4,
        ),
        (
            "/upload_media_assets.json?search[upload_id]=1",
            vec![11, 12],
            4,
        ),
        ("/upload_media_assets.json?search[upload_id]=5", vec![], 3),
    ] {
        reads.store(0, Ordering::SeqCst);
        let items = get(&routes, path, &auth).await;
        assert_eq!(reads.load(Ordering::SeqCst), expected_reads, "{path}");
        let actual: Vec<_> = items
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect();
        assert_eq!(actual, ids, "{path}");
    }
    reads.store(0, Ordering::SeqCst);
    let upload = get(&routes, "/uploads/1.json", &auth).await;
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    assert_eq!(upload["media_asset_count"], 2);
    for path in [
        "/uploads/999.json",
        "/upload_media_assets.json?search[upload_id]=999",
    ] {
        let (status, _, _) = request(
            &routes,
            "GET",
            path,
            vec![],
            "",
            &[("authorization", &auth)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    app_pool.close().await;
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "requires PostgreSQL, an S3 test endpoint, ffmpeg, and libvips"]
async fn publication_failure_repeats_media_work_and_manual_retry_gets_a_fresh_task(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let config = Arc::new(Config {
        owner_api_key: "test-key".into(),
        s3_endpoint: std::env::var("TEST_S3_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:9000".into()),
        s3_bucket: format!("test-{}", uuid::Uuid::new_v4()),
        aws_access_key_id: std::env::var("TEST_S3_ACCESS_KEY")
            .unwrap_or_else(|_| "local-access-key".into()),
        aws_secret_access_key: std::env::var("TEST_S3_SECRET_KEY")
            .unwrap_or_else(|_| "local-secret-key".into()),
        allow_private_source_urls: true,
        ..Config::default()
    });
    let app = App {
        storage: Storage::new(&config).unwrap(),
        config,
        db: pool.clone(),
    };
    app.storage.create_test_bucket().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    generate(&path, "png").await;
    let metadata = crate::media::inspect(&path).await.unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = format!("http://{}/image", listener.local_addr().unwrap());
    let source_routes = Router::new().route(
        "/image",
        axum::routing::get({
            let requests = requests.clone();
            move || {
                requests.fetch_add(1, Ordering::SeqCst);
                let bytes = bytes.clone();
                async move { bytes }
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, source_routes).await.unwrap() });
    let routes = router(app.clone());
    let auth = format!("Basic {}", STANDARD.encode("owner:test-key"));
    let (status, _, accepted) = request(
        &routes,
        "POST",
        "/uploads.json",
        json!({"upload":{"source":source}}).to_string().into_bytes(),
        "application/json",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload_id = accepted["id"].as_i64().unwrap();
    let item_id = accepted["upload_media_assets"][0]["id"].as_i64().unwrap();
    let old_task = queries::get_processing_item(&pool, item_id)
        .await
        .unwrap()
        .task_id
        .unwrap();
    sqlx::query("ALTER TABLE posts ADD CONSTRAINT reject_publish CHECK(false) NOT VALID")
        .execute(&pool)
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let workers = tokio::spawn(queue::run(app.clone(), stop.clone()));
    let failed = await_upload(&routes, &auth, upload_id).await;
    assert_eq!(failed["status"], "error", "{failed}");
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM apalis.jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_assets")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    // There is no publication checkpoint: remove a generated preview and require
    // the fresh execution to refetch the source and regenerate it.
    app.storage
        .delete(&format!("media/{}/v1/180x180.jpg", metadata.sha256))
        .await
        .unwrap();
    sqlx::query("ALTER TABLE posts DROP CONSTRAINT reject_publish")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _, _) = request(
        &routes,
        "POST",
        &format!("/uploads/{upload_id}/retry"),
        vec![],
        "",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new_task = queries::get_processing_item(&pool, item_id)
        .await
        .unwrap()
        .task_id
        .unwrap();
    assert_ne!(old_task, new_task);
    let completed = await_upload(&routes, &auth, upload_id).await;
    assert_eq!(completed["status"], "completed", "{completed}");
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    app.storage
        .download(
            &format!("media/{}/v1/180x180.jpg", metadata.sha256),
            &directory.path().join("preview"),
            1_000_000,
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM apalis.jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    worker::process(&app, item_id, &old_task).await.unwrap();
    stop.cancel();
    workers.await.unwrap().unwrap();
    server.abort();
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "requires PostgreSQL, an S3 test endpoint, ffmpeg, and libvips"]
async fn durable_uploads_search_edits_favorites_and_sessions(pool: PgPool) {
    let bucket = format!("test-{}", uuid::Uuid::new_v4());
    let config = Config {
        database_url: "postgres://localhost/test".into(),
        owner_api_key: "test-key".into(),
        s3_endpoint: std::env::var("TEST_S3_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:9000".into()),
        s3_bucket: bucket,
        aws_access_key_id: std::env::var("TEST_S3_ACCESS_KEY")
            .unwrap_or_else(|_| "local-access-key".into()),
        aws_secret_access_key: std::env::var("TEST_S3_SECRET_KEY")
            .unwrap_or_else(|_| "local-secret-key".into()),
        allow_private_source_urls: true,
        ..Config::default()
    }
    .validate(None)
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
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM apalis.jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        8
    );
    // Simulate a process disappearing after claiming a durable input.
    sqlx::raw_sql("INSERT INTO apalis.workers(id,worker_type,storage_name,last_seen) VALUES('dead-worker','uploads','PgStorage',now()-interval '1 hour');
        UPDATE apalis.jobs SET status='Running',lock_by='dead-worker',attempts=1 WHERE id=(SELECT min(id) FROM apalis.jobs)").execute(&pool).await.unwrap();
    let stop = CancellationToken::new();
    let workers = tokio::spawn(queue::run(app.clone(), stop.clone()));
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
    // Simulate a crash after object-copy, before the DB completion commit. The
    // orphaned copy is tracked; retrying uses a different immutable destination.
    let (status, _, recovery) = request(
        &routes,
        "POST",
        "/uploads/direct",
        serde_json::to_vec(&json!({"files":[{"filename":"recovery.png","size":files[1].1.len()}]}))
            .unwrap(),
        "application/json",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recovery_id = recovery["id"].as_i64().unwrap();
    let recovery_item = recovery["upload_media_assets"][0]["id"].as_i64().unwrap();
    let (status, _, recovery_ticket) = request(
        &routes,
        "POST",
        &format!("/uploads/{recovery_id}/items/{recovery_item}/ticket"),
        vec![],
        "",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let incoming = recovery_ticket["fields"]["key"].as_str().unwrap();
    let temp = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(temp.path(), &files[1].1).await.unwrap();
    app.storage
        .put(incoming, temp.path(), "application/octet-stream")
        .await
        .unwrap();
    let staging = format!("staging/{}", uuid::Uuid::new_v4());
    queries::create_upload_snapshot(
        &pool,
        queries::CreateUploadSnapshotParams {
            upload_item_id: recovery_item,
            object_key: &staging,
        },
    )
    .await
    .unwrap();
    app.storage
        .snapshot(incoming, &staging, files[1].1.len() as i64)
        .await
        .unwrap();
    let (status, _, _) = request(
        &routes,
        "POST",
        &format!("/uploads/{recovery_id}/items/{recovery_item}/complete"),
        vec![],
        "",
        &[("authorization", &auth)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let accepted: String = sqlx::query_scalar("SELECT staging_key FROM upload_items WHERE id=$1")
        .bind(recovery_item)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(accepted, staging);
    // Model the timed-out first copy completing late with different bytes.
    tokio::fs::write(temp.path(), vec![b'x'; files[1].1.len()])
        .await
        .unwrap();
    app.storage
        .put(&staging, temp.path(), "application/octet-stream")
        .await
        .unwrap();
    app.storage
        .download(&accepted, temp.path(), 2097152)
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(temp.path()).await.unwrap(), files[1].1);
    assert_eq!(
        await_upload(&routes, &auth, recovery_id).await["status"],
        "completed"
    );
    // Unexpired capabilities stay tracked even after publication and cleanup.
    worker::cleanup(&app).await.unwrap();
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_grants WHERE object_key=$1")
        .bind(incoming)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(grants, 1);
    sqlx::query(
        "UPDATE upload_snapshots SET created_at=now()-interval '3 hours' WHERE object_key=$1",
    )
    .bind(&staging)
    .execute(&pool)
    .await
    .unwrap();
    worker::cleanup(&app).await.unwrap();
    assert!(
        app.storage
            .download(&staging, temp.path(), 2097152)
            .await
            .is_err()
    );
    let snapshots: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_snapshots WHERE object_key=$1")
            .bind(&staging)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(snapshots, 0);
    sqlx::query("UPDATE upload_grants SET expires_at=now()-interval '2 hours' WHERE object_key=$1")
        .bind(incoming)
        .execute(&pool)
        .await
        .unwrap();
    worker::cleanup(&app).await.unwrap();
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_grants WHERE object_key=$1")
        .bind(incoming)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(grants, 0);
    assert!(
        app.storage
            .download(incoming, temp.path(), 2097152)
            .await
            .is_err()
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
        3
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
    // Drop the whole runtime without graceful shutdown, as with process death.
    workers.abort();
    assert!(workers.await.unwrap_err().is_cancelled());
    let interrupted_id = interrupted["id"].as_i64().unwrap();
    assert_eq!(
        get(&routes, &format!("/uploads/{interrupted_id}.json"), &auth).await["status"],
        "processing"
    );
    slow.store(false, std::sync::atomic::Ordering::SeqCst);
    sqlx::query("UPDATE apalis.workers SET last_seen=now()-interval '1 hour' WHERE id IN
        (SELECT j.lock_by FROM apalis.jobs j JOIN upload_items i ON j.id=i.task_id WHERE i.upload_id=$1)")
        .bind(interrupted_id).execute(&pool).await.unwrap();
    let stop = CancellationToken::new();
    let resumed = tokio::spawn(queue::run(app.clone(), stop.clone()));
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
