use crate::{
    App,
    db::{generated as queries, optional},
    error::{Error, Result},
};
use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}
fn credentials_ok(app: &App, login: &str, key: &str) -> bool {
    let login_matches =
        Sha256::digest(login.as_bytes()).ct_eq(&Sha256::digest(app.config.owner_login.as_bytes()));
    let key_matches =
        Sha256::digest(key.as_bytes()).ct_eq(&Sha256::digest(app.config.owner_api_key.as_bytes()));
    bool::from(login_matches & key_matches)
}
pub fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix("booru_session=")
                .map(str::to_owned)
        })
}

pub async fn require_auth(State(app): State<App>, request: Request, next: Next) -> Response {
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) && request.headers().get(header::ORIGIN).is_some_and(|origin| {
        origin.to_str().ok() != Some(app.config.base_url.trim_end_matches('/'))
    }) {
        return Error(StatusCode::FORBIDDEN, "invalid origin".into()).into_response();
    }
    let mut authorized = false;
    if let Some(value) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
    {
        if let Ok(decoded) = STANDARD.decode(value)
            && let Ok(decoded) = String::from_utf8(decoded)
            && let Some((login, key)) = decoded.split_once(':')
        {
            authorized = credentials_ok(&app, login, key);
        }
    } else if let Some(query) = request.uri().query() {
        let params: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
        if let (Some(login), Some(key)) = (params.get("login"), params.get("api_key")) {
            authorized = credentials_ok(&app, login, key);
        }
    }
    if !authorized && let Some(token) = cookie(request.headers()) {
        let csrf = optional(queries::get_session(&app.db, &hash(&token)).await)
            .map(|row| row.map(|row| row.csrf_token));
        match csrf {
            Ok(Some(csrf)) => {
                if !matches!(
                    *request.method(),
                    axum::http::Method::GET
                        | axum::http::Method::HEAD
                        | axum::http::Method::OPTIONS
                ) && request
                    .headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok())
                    != Some(&csrf)
                {
                    return Error(StatusCode::FORBIDDEN, "invalid CSRF token".into())
                        .into_response();
                }
                authorized = true;
            }
            Err(error) => return Error::from(error).into_response(),
            _ => {}
        }
    }
    if authorized {
        next.run(request).await
    } else {
        let mut response =
            Error(StatusCode::UNAUTHORIZED, "authentication required".into()).into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"booru-thingy\"".parse().unwrap(),
        );
        response
    }
}

#[derive(Deserialize)]
pub struct Login {
    login: String,
    api_key: String,
}
pub async fn login(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response> {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        && origin.trim_end_matches('/') != app.config.base_url.trim_end_matches('/')
    {
        return Err(Error(StatusCode::FORBIDDEN, "invalid origin".into()));
    }
    if !credentials_ok(&app, &input.login, &input.api_key) {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "invalid login or API key".into(),
        ));
    }
    let token = token();
    let csrf = self::token();
    queries::create_session(
        &app.db,
        queries::CreateSessionParams {
            token_hash: &hash(&token),
            csrf_token: &csrf,
        },
    )
    .await?;
    let secure = if app.config.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let mut response =
        Json(json!({"csrf_token":csrf,"name":app.config.owner_login})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("booru_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800{secure}")
            .parse()
            .unwrap(),
    );
    Ok(response)
}
pub async fn session(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    let csrf = if let Some(token) = cookie(&headers) {
        optional(queries::get_session(&app.db, &hash(&token)).await)?.map(|row| row.csrf_token)
    } else {
        None
    };
    Ok(Json(
        json!({"name":app.config.owner_login,"csrf_token":csrf}),
    ))
}
pub async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response> {
    if let Some(token) = cookie(&headers) {
        queries::delete_session(&app.db, &hash(&token)).await?;
    }
    let mut response = Json(json!({"success":true})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        "booru_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
            .parse()
            .unwrap(),
    );
    Ok(response)
}
