use crate::{
    App,
    api::{self, Params},
    db::{generated as queries, optional},
    error::{Error, Result},
};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/uploads.json", post(create).get(index))
        .route("/uploads/direct", post(reserve))
        .route("/uploads/{id}/items/{item_id}/ticket", post(ticket))
        .route("/uploads/{id}/items/{item_id}/complete", post(complete))
        .route("/uploads/{id}", get(show))
        .route("/uploads/{id}/retry", post(retry))
        .route("/users/{id}/uploads.json", get(user_index))
        .route("/upload_media_assets.json", get(items))
}
async fn create(State(app): State<App>, request: Request) -> Result<Json<Value>> {
    let multipart = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    if multipart {
        return Err(Error::bad("use direct signed uploads for files"));
    }
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 16384)
        .await
        .map_err(|_| Error::bad("upload form too large"))?;
    let fields = api::input(&parts.headers, &bytes)?;
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
    let mut tx = app.db.begin().await?;
    let upload_id = queries::create_upload(&mut tx).await?.id;
    queries::update_upload_metadata(
        &mut tx,
        queries::UpdateUploadMetadataParams {
            id: upload_id,
            source,
            tag_string: &tags,
            rating,
            referer_url: fields
                .get("upload[referer_url]")
                .map(String::as_str)
                .unwrap_or(""),
        },
    )
    .await?;
    let item = queries::create_source_item(
        &mut tx,
        queries::CreateSourceItemParams {
            upload_id,
            source_url: source,
            filename: url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or("import"),
        },
    )
    .await?;
    crate::worker::enqueue(&mut tx, item.id).await?;
    tx.commit().await?;
    Ok(Json(upload_object(&app, upload_id).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    files: Vec<ReservedFile>,
    #[serde(default)]
    tags: String,
    #[serde(default = "general_rating")]
    rating: String,
    #[serde(default)]
    source: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservedFile {
    filename: String,
    size: u64,
}
fn general_rating() -> String {
    "g".into()
}

async fn reserve(State(app): State<App>, Json(input): Json<Reservation>) -> Result<Json<Value>> {
    if input.files.is_empty() || input.files.len() > 32 {
        return Err(Error::bad("choose files (maximum 32 files per upload)"));
    }
    let mut total = 0_u64;
    for file in &input.files {
        if file.size == 0 {
            return Err(Error::bad("empty files are not supported"));
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| Error::bad("invalid file sizes"))?;
        if total > app.config.max_upload_bytes {
            return Err(Error(
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                "upload exceeds configured size limit".into(),
            ));
        }
        if file.filename.is_empty() || file.filename.chars().count() > 255 {
            return Err(Error::bad("filename must contain 1..255 characters"));
        }
    }
    let tags = api::normalize_tags(if input.tags.trim().is_empty() {
        "tagme"
    } else {
        &input.tags
    })?
    .into_iter()
    .collect::<Vec<_>>()
    .join(" ");
    let rating = api::rating(&input.rating)?;
    let mut tx = app.db.begin().await?;
    let upload_id = queries::create_upload(&mut tx).await?.id;
    queries::update_upload_metadata(
        &mut tx,
        queries::UpdateUploadMetadataParams {
            id: upload_id,
            source: &input.source,
            tag_string: &tags,
            rating,
            referer_url: "",
        },
    )
    .await?;
    for file in &input.files {
        queries::create_direct_item(
            &mut tx,
            queries::CreateDirectItemParams {
                upload_id,
                filename: &file.filename,
                expected_size: Some(file.size as i64),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(upload_object(&app, upload_id).await?))
}

async fn ticket(
    State(app): State<App>,
    Path((upload, item)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let upload_id = api::id(&upload)?;
    let item_id = api::id(&item)?;
    let mut tx = app.db.begin().await?;
    let row = queries::lock_direct_item(
        &mut tx,
        queries::LockDirectItemParams {
            id: item_id,
            upload_id,
        },
    )
    .await?;
    if row.status != "receiving" {
        return Err(Error(
            axum::http::StatusCode::CONFLICT,
            "item no longer accepts transfers".into(),
        ));
    }
    if queries::count_upload_grants(&mut tx, item_id).await?.count >= 8 {
        return Err(Error::bad(
            "too many transfer attempts; reserve a new upload",
        ));
    }
    let key = format!("incoming/{}", uuid::Uuid::new_v4());
    let expires =
        chrono::Utc::now() + chrono::Duration::seconds(app.config.upload_ticket_ttl_seconds.into());
    queries::create_upload_grant(
        &mut tx,
        queries::CreateUploadGrantParams {
            upload_item_id: item_id,
            object_key: &key,
            expires_at: expires,
        },
    )
    .await?;
    queries::touch_upload_item(&mut tx, item_id).await?;
    tx.commit().await?;
    Ok(Json(app.storage.upload_ticket(
        &key,
        row.expected_size.unwrap(),
        expires,
    )))
}

async fn complete(
    State(app): State<App>,
    Path((upload, item)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let upload_id = api::id(&upload)?;
    let item_id = api::id(&item)?;
    let mut tx = app.db.begin().await?;
    let row = queries::lock_direct_item(
        &mut tx,
        queries::LockDirectItemParams {
            id: item_id,
            upload_id,
        },
    )
    .await?;
    if row.status == "receiving" {
        let grant = optional(queries::latest_upload_grant(&mut tx, item_id).await)?
            .ok_or_else(|| Error::bad("request an upload ticket first"))?;
        // Completion may follow a transfer that started before its ticket expired.
        if grant.expires_at + chrono::Duration::hours(1) < chrono::Utc::now() {
            return Err(Error::bad("upload ticket expired; retry the transfer"));
        }
        let staging = format!("staging/{}", uuid::Uuid::new_v4());
        queries::create_upload_snapshot(
            &mut tx,
            queries::CreateUploadSnapshotParams {
                upload_item_id: item_id,
                object_key: &staging,
            },
        )
        .await?;
        queries::touch_upload_item(&mut tx, item_id).await?;
        // Persist the unique destination before issuing the copy. A timeout or
        // crash can leave an object, but can never overwrite another attempt.
        tx.commit().await?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            app.storage
                .snapshot(&grant.object_key, &staging, row.expected_size.unwrap()),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(Error(axum::http::StatusCode::CONFLICT, error.to_string()));
            }
            Err(_) => {
                return Err(Error(
                    axum::http::StatusCode::CONFLICT,
                    "upload snapshot timed out; retry completion".into(),
                ));
            }
        }
        tx = app.db.begin().await?;
        let current = queries::lock_direct_item(
            &mut tx,
            queries::LockDirectItemParams {
                id: item_id,
                upload_id,
            },
        )
        .await?;
        if current.status == "receiving" {
            let latest = optional(queries::latest_upload_grant(&mut tx, item_id).await)?;
            if latest.as_ref().map(|g| &g.object_key) != Some(&grant.object_key) {
                return Err(Error(
                    axum::http::StatusCode::CONFLICT,
                    "transfer ticket changed; retry completion".into(),
                ));
            }
            queries::accept_direct_item(
                &mut tx,
                queries::AcceptDirectItemParams {
                    id: item_id,
                    staging_key: Some(&staging),
                },
            )
            .await?;
            crate::worker::enqueue(&mut tx, item_id).await?;
            queries::touch_upload(&mut tx, upload_id).await?;
        } else if current.accepted_at.is_none() {
            return Err(Error(
                axum::http::StatusCode::CONFLICT,
                "item expired during completion".into(),
            ));
        }
    } else if row.status == "error" && row.accepted_at.is_none() {
        return Err(Error(
            axum::http::StatusCode::CONFLICT,
            "item failed; retry transfer or processing".into(),
        ));
    }
    tx.commit().await?;
    Ok(Json(upload_object(&app, upload_id).await?))
}
async fn upload_object(app: &App, upload_id: i64) -> Result<Value> {
    let upload = queries::get_upload(&app.db, upload_id).await?.payload;
    Ok(upload_objects(app, vec![upload]).await?.remove(0))
}
async fn upload_objects(app: &App, uploads: Vec<Value>) -> Result<Vec<Value>> {
    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<_> = uploads.iter().map(|u| u["id"].as_i64().unwrap()).collect();
    let records = queries::list_upload_items(&app.db, ids).await?;
    let asset_ids: Vec<_> = records
        .iter()
        .filter_map(|row| row.media_asset_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut assets = HashMap::new();
    if !asset_ids.is_empty() {
        for row in queries::get_media_assets(&app.db, asset_ids).await? {
            assets.insert(row.id, api::serialize_asset(app, row.payload).await?);
        }
    }
    let owner = api::owner(app).await?;
    let mut items_by_upload: HashMap<_, Vec<_>> = HashMap::new();
    for row in records {
        items_by_upload.entry(row.upload_id).or_default().push(row);
    }
    uploads
        .into_iter()
        .map(|upload| {
            let id = upload["id"].as_i64().unwrap();
            serialize_upload(
                upload,
                items_by_upload.remove(&id).unwrap_or_default(),
                &assets,
                &owner,
            )
        })
        .collect()
}
fn serialize_upload(
    mut upload: Value,
    records: Vec<queries::ListUploadItemsRow>,
    media_assets: &HashMap<i64, Value>,
    owner: &Value,
) -> Result<Value> {
    let upload_id = upload["id"].as_i64().unwrap();
    let mut items = Vec::new();
    let mut assets = Vec::new();
    let mut posts = Vec::new();
    let mut pending = false;
    let mut failed = false;
    let mut errors = Vec::new();
    for row in records {
        let status = row.status;
        pending |= !matches!(status.as_str(), "completed" | "error");
        failed |= status == "error";
        let error = (!row.error.is_empty()).then_some(row.error);
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        let asset_id = row.media_asset_id;
        let post_id = row.post_id;
        let mut item = json!({"id":row.id,"upload_id":upload_id,"media_asset_id":asset_id,"status":status,"error":error,"source_url":row.source_url,"page_url":upload["source"],"filename":row.filename,"duplicate":row.duplicate,"post_id":post_id,"created_at":row.created,"updated_at":row.updated});
        if let Some(asset_id) = asset_id {
            let mut asset = media_assets
                .get(&asset_id)
                .ok_or_else(Error::missing)?
                .clone();
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
    upload["uploader"] = owner.clone();
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
    let is_posted = match params.get("search[is_posted]").map(String::as_str) {
        Some("true" | "1") => Some(true),
        Some("false" | "0") => Some(false),
        Some(_) => return Err(Error::bad("invalid is_posted filter")),
        None => None,
    };
    let status = params.get("search[status]").map(String::as_str);
    if status.is_some_and(|s| !matches!(s, "pending" | "processing" | "completed" | "error")) {
        return Err(Error::bad("invalid status filter"));
    }
    if params
        .get("search[ai_tags_match]")
        .is_some_and(|v| !v.is_empty())
    {
        return Ok(Json(json!([])));
    }
    let uploads = queries::list_uploads(
        &app.db,
        queries::ListUploadsParams {
            is_posted,
            status,
            ascending: params.get("search[order]").is_some_and(|v| v == "id_asc"),
            result_limit: limit,
            result_offset: api::offset(&params, limit),
        },
    )
    .await?
    .into_iter()
    .map(|r| r.payload)
    .collect();
    Ok(Json(json!(upload_objects(&app, uploads).await?)))
}
async fn items(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let uploads = if let Some(upload_id) = params.get("search[upload_id]") {
        vec![
            queries::get_upload(&app.db, api::id(upload_id)?)
                .await?
                .payload,
        ]
    } else {
        queries::recent_uploads(&app.db, api::limit(&params, 20))
            .await?
            .into_iter()
            .map(|row| row.payload)
            .collect()
    };
    let mut output = Vec::new();
    for upload in upload_objects(&app, uploads).await? {
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
    let items = queries::lock_retry_upload_items(&mut tx, upload_id).await?;
    for item in items {
        if queries::get_upload_item_state(&mut tx, item.id)
            .await?
            .status
            == "error"
        {
            crate::worker::enqueue(&mut tx, item.id).await?;
        }
    }
    tx.commit().await?;
    Ok(Json(upload_object(&app, upload_id).await?))
}
