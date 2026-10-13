use crate::{App, db::generated as queries};
use axum::{
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
};
use include_dir::{Dir, include_dir};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

pub async fn home() -> Redirect {
    Redirect::temporary("/uploads/new")
}
pub async fn upload_page(State(app): State<App>) -> Html<String> {
    Html(
        include_str!("../templates/upload.html")
            .replace("{{MAX_BYTES}}", &app.config.max_upload_bytes.to_string()),
    )
}
pub async fn asset(Path(path): Path<String>) -> Response {
    let content_type = match std::path::Path::new(&path)
        .extension()
        .and_then(|s| s.to_str())
    {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match ASSETS.get_file(&path) {
        Some(file) => ([(header::CONTENT_TYPE, content_type)], file.contents()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
pub async fn ready(State(app): State<App>) -> impl IntoResponse {
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        queries::check_database_ready(&app.db)
            .await
            .map_err(anyhow::Error::from)?;
        app.storage.ready().await
    })
    .await;
    if matches!(result, Ok(Ok(()))) {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}
pub async fn headers(State(app): State<App>, request: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let method = request.method().clone();
    // Log the route path only, never query credentials, cookies, or media URLs.
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    let storage_origin = app.storage.upload_origin();
    headers.insert("content-security-policy",format!("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob: https: http:; media-src blob: https: http:; connect-src 'self' {storage_origin}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'").parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    tracing::info!(%method,%path,status=response.status().as_u16(),elapsed_ms=start.elapsed().as_millis(),"request");
    response
}
pub fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn embedded_assets_serve_root_and_nested_modules() {
        let router = Router::new().route("/assets/{*path}", get(asset));
        for (path, content_type) in [
            ("app.css", "text/css; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            (
                "components/upload-page.js",
                "text/javascript; charset=utf-8",
            ),
            ("van-1.5.5.min.js", "text/javascript; charset=utf-8"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/assets/{path}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
        }
        for path in [
            "missing.js",
            "components/missing.js",
            "van-LICENSE.txt",
            "../src/main.rs",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/assets/{path}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}
