use crate::{App, db::generated as queries};
use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
};
pub async fn home() -> Redirect {
    Redirect::temporary("/uploads/new")
}
pub async fn upload_page(State(app): State<App>) -> Html<String> {
    Html(
        include_str!("../templates/upload.html")
            .replace("{{MAX_BYTES}}", &app.config.max_upload_bytes.to_string()),
    )
}
pub async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/app.css"),
    )
}
pub async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../assets/app.js"),
    )
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
