use crate::{
    App,
    api::{self, Params},
    error::{Error, Result},
};
use axum::{
    Json, Router,
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Row};
use tokio::io::AsyncWriteExt;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/uploads.json", post(create).get(index))
        .route("/uploads/{id}", get(show))
        .route("/uploads/{id}/retry", post(retry))
        .route("/users/{id}/uploads.json", get(user_index))
        .route("/upload_media_assets.json", get(items))
}
async fn create(State(app): State<App>, request: Request) -> Result<Json<Value>> {
    let upload_id: i64 = sqlx::query_scalar("INSERT INTO uploads DEFAULT VALUES RETURNING id")
        .fetch_one(&app.db)
        .await?;
    let multipart = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    let result = if multipart {
        match Multipart::from_request(request, &app).await {
            Ok(mut multipart) => receive(&app, upload_id, &mut multipart).await,
            Err(_) => Err(Error::bad("invalid multipart upload")),
        }
    } else {
        let (parts, body) = request.into_parts();
        match axum::body::to_bytes(body, 16384).await {
            Ok(bytes) => match api::input(&parts.headers, &bytes) {
                Ok(fields) => finalize(&app, upload_id, &fields, 0).await,
                Err(error) => Err(error),
            },
            Err(_) => Err(Error::bad("upload form too large")),
        }
    };
    if let Err(error) = result {
        sqlx::query("UPDATE upload_items SET status='error',error='Upload interrupted or rejected',updated_at=now() WHERE upload_id=$1 AND status='receiving'").bind(upload_id).execute(&app.db).await?;
        return Err(error);
    }
    Ok(Json(upload_object(&app, upload_id).await?))
}
async fn receive(app: &App, upload_id: i64, multipart: &mut Multipart) -> Result<()> {
    let mut fields = Params::new();
    let mut count = 0;
    let mut total = 0_u64;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| Error::bad("invalid multipart upload"))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if let Some(filename) = field.file_name().map(str::to_owned) {
            if !matches!(
                name.as_str(),
                "upload[files]" | "upload[files][]" | "upload[file]"
            ) {
                return Err(Error::bad("unexpected file field"));
            }
            count += 1;
            if count > 32 {
                return Err(Error::bad("maximum 32 files per upload"));
            }
            let key = format!("staging/{}", uuid::Uuid::new_v4());
            let item_id: i64 = sqlx::query_scalar("INSERT INTO upload_items(upload_id,filename,staging_key,status) VALUES ($1,$2,$3,'receiving') RETURNING id").bind(upload_id).bind(filename.chars().take(255).collect::<String>()).bind(&key).fetch_one(&app.db).await?;
            let temp = tempfile::NamedTempFile::new().map_err(anyhow::Error::from)?;
            let mut file = tokio::fs::File::create(temp.path())
                .await
                .map_err(anyhow::Error::from)?;
            let mut size = 0;
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| Error::bad("upload stream interrupted"))?
            {
                size += chunk.len() as u64;
                total += chunk.len() as u64;
                if total > app.config.max_upload_bytes {
                    return Err(Error(
                        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                        "upload exceeds configured size limit".into(),
                    ));
                }
                file.write_all(&chunk).await.map_err(anyhow::Error::from)?;
            }
            if size == 0 {
                return Err(Error::bad("empty files are not supported"));
            }
            file.flush().await.map_err(anyhow::Error::from)?;
            app.storage
                .put(&key, temp.path(), "application/octet-stream")
                .await?;
            // Keep receiving until every input and its metadata has been accepted.
            sqlx::query("UPDATE upload_items SET updated_at=now() WHERE id=$1")
                .bind(item_id)
                .execute(&app.db)
                .await?;
        } else {
            let mut value = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| Error::bad("invalid form field"))?
            {
                if value.len() + chunk.len() > 16384 {
                    return Err(Error::bad("form field too large"));
                }
                value.extend_from_slice(&chunk);
            }
            fields.insert(
                name,
                String::from_utf8(value).map_err(|_| Error::bad("form fields must be UTF-8"))?,
            );
        }
    }
    finalize(app, upload_id, &fields, count).await
}
async fn finalize(app: &App, upload_id: i64, fields: &Params, count: usize) -> Result<()> {
    let source = fields
        .get("upload[source]")
        .map(String::as_str)
        .unwrap_or("");
    let tags = api::normalize_tags(
        fields
            .get("upload[tag_string]")
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("tagme"),
    )?
    .into_iter()
    .collect::<Vec<_>>()
    .join(" ");
    let rating = api::rating(
        fields
            .get("upload[rating]")
            .map(String::as_str)
            .unwrap_or("g"),
    )?;
    let mut tx = app.db.begin().await?;
    sqlx::query("UPDATE uploads SET source=$2,tag_string=$3,rating=$4,referer_url=$5,updated_at=now() WHERE id=$1")
        .bind(upload_id).bind(source).bind(tags).bind(rating).bind(fields.get("upload[referer_url]").cloned().unwrap_or_default()).execute(&mut *tx).await?;
    if count == 0 {
        let url = url::Url::parse(source)
            .map_err(|_| Error::bad("choose files or enter a direct media URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::bad(
                "source must be an HTTP(S) URL without credentials",
            ));
        }
        sqlx::query("INSERT INTO upload_items(upload_id,source_url,filename) VALUES ($1,$2,$3)")
            .bind(upload_id)
            .bind(source)
            .bind(
                url.path_segments()
                    .and_then(|mut s| s.next_back())
                    .unwrap_or("import"),
            )
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE upload_items SET status='pending',updated_at=now() WHERE upload_id=$1 AND status='receiving'").bind(upload_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO jobs(upload_item_id) SELECT id FROM upload_items WHERE upload_id=$1")
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn upload_object(app: &App, upload_id: i64) -> Result<Value> {
    let mut upload: Value = sqlx::query_scalar("SELECT to_jsonb(u) FROM uploads u WHERE id=$1")
        .bind(upload_id)
        .fetch_one(&app.db)
        .await?;
    let records = sqlx::query("SELECT *,created_at::text AS created,updated_at::text AS updated FROM upload_items WHERE upload_id=$1 ORDER BY id").bind(upload_id).fetch_all(&app.db).await?;
    let mut items = Vec::new();
    let mut assets = Vec::new();
    let mut posts = Vec::new();
    let mut pending = false;
    let mut failed = false;
    let mut errors = Vec::new();
    for row in records {
        let status: String = row.get("status");
        pending |= !matches!(status.as_str(), "completed" | "error");
        failed |= status == "error";
        let error: Option<String> = row.get("error");
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        let asset_id: Option<i64> = row.get("media_asset_id");
        let post_id: Option<i64> = row.get("post_id");
        let mut item = json!({"id":row.get::<i64,_>("id"),"upload_id":upload_id,"media_asset_id":asset_id,"status":status,"error":error,"source_url":row.get::<String,_>("source_url"),"page_url":upload["source"],"filename":row.get::<String,_>("filename"),"duplicate":row.get::<bool,_>("duplicate"),"post_id":post_id,"created_at":row.get::<String,_>("created"),"updated_at":row.get::<String,_>("updated")});
        if let Some(asset_id) = asset_id {
            let mut asset = api::asset_by_id(app, asset_id).await?;
            if let Some(post_id) = post_id {
                asset["post"] = json!({"id":post_id});
                posts.push(json!({"id":post_id}));
            }
            item["media_asset"] = asset.clone();
            assets.push(asset);
        }
        items.push(item);
    }
    upload["status"] = json!(if pending {
        "processing"
    } else if failed || items.is_empty() {
        "error"
    } else {
        "completed"
    });
    upload["error"] = if errors.is_empty() {
        Value::Null
    } else {
        json!(errors.join("; "))
    };
    upload["uploader_id"] = json!(1);
    upload["uploader"] = api::owner(app).await?;
    upload["media_asset_count"] = json!(assets.len());
    upload["upload_media_assets"] = json!(items);
    upload["media_assets"] = json!(assets);
    upload["posts"] = json!(posts);
    Ok(upload)
}
async fn show(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    Ok(Json(upload_object(&app, api::id(&value)?).await?))
}
async fn index(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    list(app, params).await
}
async fn user_index(
    State(app): State<App>,
    Path(value): Path<String>,
    Query(params): Query<Params>,
) -> Result<Json<Value>> {
    if api::id(&value)? != 1 {
        return Err(Error::missing());
    }
    list(app, params).await
}
async fn list(app: App, params: Params) -> Result<Json<Value>> {
    let limit = api::limit(&params, 20);
    let mut query = QueryBuilder::<Postgres>::new("SELECT u.id FROM uploads u WHERE true");
    if let Some(posted) = params.get("search[is_posted]") {
        match posted.as_str() {
            "true" | "1" => {
                query.push(" AND EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.post_id IS NOT NULL)");
            }
            "false" | "0" => {
                query.push(" AND NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.post_id IS NOT NULL)");
            }
            _ => return Err(Error::bad("invalid is_posted filter")),
        }
    }
    if let Some(status) = params.get("search[status]") {
        match status.as_str() {
            "pending" | "processing" => {
                query.push(" AND EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.status IN ('receiving','pending','processing'))");
            }
            "completed" => {
                query.push(" AND EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id) AND NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.status<>'completed')");
            }
            "error" => {
                query.push(" AND (NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id) OR EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.status='error')) AND NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.status IN ('receiving','pending','processing'))");
            }
            _ => return Err(Error::bad("invalid status filter")),
        }
    }
    if params
        .get("search[ai_tags_match]")
        .is_some_and(|v| !v.is_empty())
    {
        query.push(" AND false");
    }
    query.push(
        if params.get("search[order]").is_some_and(|v| v == "id_asc") {
            " ORDER BY u.id ASC"
        } else {
            " ORDER BY u.id DESC"
        },
    );
    query
        .push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(api::offset(&params, limit));
    let ids: Vec<i64> = query.build_query_scalar().fetch_all(&app.db).await?;
    let mut output = Vec::new();
    for id in ids {
        output.push(upload_object(&app, id).await?);
    }
    Ok(Json(json!(output)))
}
async fn items(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let ids: Vec<i64> = if let Some(upload_id) = params.get("search[upload_id]") {
        vec![api::id(upload_id)?]
    } else {
        sqlx::query_scalar("SELECT id FROM uploads ORDER BY id DESC LIMIT $1")
            .bind(api::limit(&params, 20))
            .fetch_all(&app.db)
            .await?
    };
    let mut output = Vec::new();
    for id in ids {
        let upload = upload_object(&app, id).await?;
        output.extend(
            upload["upload_media_assets"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
    }
    Ok(Json(json!(output)))
}
async fn retry(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    let upload_id = api::id(&value)?;
    let mut tx = app.db.begin().await?;
    let ids: Vec<i64> = sqlx::query_scalar("UPDATE upload_items SET status='pending',error=NULL,updated_at=now() WHERE upload_id=$1 AND status='error' AND (staging_key IS NOT NULL OR source_url <> '') RETURNING id").bind(upload_id).fetch_all(&mut *tx).await?;
    for id in ids {
        sqlx::query("INSERT INTO jobs(upload_item_id) VALUES ($1) ON CONFLICT(upload_item_id) DO UPDATE SET attempts=0,available_at=now(),lease_until=NULL,lease_token=NULL,completed_at=NULL,last_error=NULL").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(upload_object(&app, upload_id).await?))
}
