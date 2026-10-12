use crate::{
    App,
    error::{Error, Result},
    search::{Search, escape_like},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use std::collections::{BTreeSet, HashMap};

pub type Params = HashMap<String, String>;
pub fn routes() -> Router<App> {
    let mut routes = Router::new()
        .route("/profile.json", get(profile))
        .route("/users.json", get(users))
        .route("/users/{id}", get(user).patch(unsupported))
        .route("/posts.json", get(posts).post(create_post))
        .route("/posts/{id}", get(show_post).put(edit_post))
        .route("/counts/posts.json", get(count_posts))
        .route("/tags.json", get(tags))
        .route("/autocomplete.json", get(autocomplete))
        .route("/related_tag.json", get(related))
        .route("/favorites.json", get(favorites).post(add_favorite))
        .route("/favorites/{id}", axum::routing::delete(delete_favorite))
        .route("/posts/{id}/favorites.json", get(post_favorites))
        .route("/media_assets.json", get(assets))
        .route("/wiki_pages/{title}", get(wiki))
        .route("/pools/{id}", get(|| async { "" }))
        .route("/source.json", get(source))
        .route("/dtext_preview", post(dtext));
    for path in [
        "/ai_tags.json",
        "/post_votes.json",
        "/comments.json",
        "/comment_votes.json",
        "/saved_searches.json",
        "/favorite_groups.json",
        "/notes.json",
        "/post_versions.json",
        "/pools.json",
        "/forum_topics.json",
        "/forum_posts.json",
        "/forum_post_votes.json",
        "/dmails.json",
        "/artists.json",
        "/artist_urls.json",
        "/artist_commentaries.json",
        "/reports/posts.json",
        "/explore/posts/popular.json",
        "/explore/posts/viewed.json",
        "/explore/posts/searches.json",
        "/iqdb_queries.json",
    ] {
        routes = routes.route(path, get(empty));
    }
    for path in [
        "/comments.json",
        "/saved_searches.json",
        "/favorite_groups.json",
        "/notes.json",
        "/posts/{id}/votes.json",
        "/comments/{id}/votes.json",
    ] {
        routes = routes.route(path, post(unsupported));
    }
    for path in [
        "/comments/{id}",
        "/saved_searches/{id}",
        "/favorite_groups/{id}",
        "/notes/{id}",
        "/post_votes/{id}",
        "/comment_votes/{id}",
        "/dmails/{id}",
    ] {
        routes = routes.route(
            path,
            axum::routing::put(unsupported)
                .patch(unsupported)
                .delete(unsupported),
        );
    }
    routes
}
pub fn id(value: &str) -> Result<i64> {
    value
        .strip_suffix(".json")
        .unwrap_or(value)
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(Error::missing)
}
pub fn limit(params: &Params, default: i64) -> i64 {
    params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .clamp(1, 200)
}
pub fn offset(params: &Params, limit: i64) -> i64 {
    params
        .get("page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .clamp(1, 1_000_000)
        .saturating_sub(1)
        * limit
}
pub fn input(headers: &HeaderMap, bytes: &Bytes) -> Result<Params> {
    if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("application/json")
    {
        let object: Value =
            serde_json::from_slice(bytes).map_err(|_| Error::bad("invalid JSON"))?;
        let mut params = Params::new();
        fn flatten(prefix: &str, value: &Value, params: &mut Params) {
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    flatten(
                        &if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}[{key}]")
                        },
                        value,
                        params,
                    );
                }
            } else {
                params.insert(
                    prefix.to_owned(),
                    value.as_str().map(str::to_owned).unwrap_or_else(|| {
                        if value.is_null() {
                            String::new()
                        } else {
                            value.to_string()
                        }
                    }),
                );
            }
        }
        flatten("", &object, &mut params);
        Ok(params)
    } else {
        serde_urlencoded::from_bytes(bytes).map_err(|_| Error::bad("invalid form"))
    }
}
pub fn rating(value: &str) -> Result<&str> {
    if ["g", "s", "q", "e"].contains(&value) {
        Ok(value)
    } else {
        Err(Error::bad("rating must be g, s, q, or e"))
    }
}
pub fn normalize_tags(value: &str) -> Result<BTreeSet<String>> {
    let tags: BTreeSet<_> = value.split_whitespace().map(str::to_lowercase).collect();
    if tags.len() > 200
        || tags
            .iter()
            .any(|t| t.len() > 128 || t.contains(':') || t.starts_with('-'))
    {
        return Err(Error::bad(
            "invalid tag string (maximum 200 tags, 128 bytes per tag)",
        ));
    }
    Ok(tags)
}

pub async fn owner(app: &App) -> Result<Value> {
    let row = sqlx::query("SELECT s.*, (SELECT count(*) FROM posts) AS uploads, (SELECT count(*) FROM favorites) AS favorites FROM user_settings s WHERE id=1").fetch_one(&app.db).await?;
    Ok(
        json!({"id":1,"name":app.config.owner_login,"level":50,"level_string":"Owner","created_at":row.get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,_>("created_at").to_rfc3339(),"updated_at":row.get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,_>("updated_at").to_rfc3339(),"post_upload_count":row.get::<i64,_>("uploads"),"favorite_count":row.get::<i64,_>("favorites"),"post_update_count":0,"note_update_count":0,"comment_count":0,"forum_post_count":0,"blacklisted_tags":row.get::<String,_>("blacklisted_tags"),"can_upload_free":true,"is_banned":false,"is_deleted":false,"show_deleted_children":true}),
    )
}
async fn profile(State(app): State<App>) -> Result<Json<Value>> {
    Ok(Json(owner(&app).await?))
}
async fn users(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    if params.get("search[id]").is_some_and(|v| v != "1")
        || params
            .get("search[name]")
            .is_some_and(|v| v != &app.config.owner_login)
    {
        return Ok(Json(json!([])));
    }
    Ok(Json(json!([owner(&app).await?])))
}
async fn user(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    if id(&value)? != 1 {
        return Err(Error::missing());
    }
    Ok(Json(owner(&app).await?))
}

const POST_SELECT: &str = "SELECT to_jsonb(p) || jsonb_build_object('md5',a.md5,'file_ext',a.file_ext,'file_size',a.file_size,'image_width',a.image_width,'image_height',a.image_height,'fav_count',(SELECT count(*) FROM favorites f WHERE f.post_id=p.id),'has_children',EXISTS(SELECT 1 FROM posts child WHERE child.parent_id=p.id),'_tags',COALESCE((SELECT jsonb_agg(jsonb_build_object('name',t.name,'category',t.category) ORDER BY t.name) FROM post_tags pt JOIN tags t ON t.id=pt.tag_id WHERE pt.post_id=p.id),'[]'::jsonb),'media_asset',to_jsonb(a) || jsonb_build_object('variants',COALESCE((SELECT jsonb_agg(to_jsonb(v) ORDER BY v.id) FROM media_variants v WHERE v.media_asset_id=a.id),'[]'::jsonb))) FROM posts p JOIN media_assets a ON a.id=p.media_asset_id";

pub async fn serialize_asset(app: &App, mut asset: Value) -> Result<Value> {
    let object = asset
        .as_object_mut()
        .ok_or_else(|| Error::bad("invalid asset"))?;
    object.remove("original_key");
    object.remove("sha256");
    object.remove("content_type");
    object.remove("processing_version");
    object.insert("is_public".into(), json!(false));
    object.insert("pixel_hash".into(), json!(""));
    if let Some(duration) = object.get("duration").and_then(Value::as_f64) {
        object.insert("duration".into(), json!(duration));
    }
    if let Some(variants) = object.get_mut("variants").and_then(Value::as_array_mut) {
        for variant in variants {
            let object = variant.as_object_mut().unwrap();
            let key = object
                .remove("object_key")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let kind = object.remove("kind").unwrap_or(Value::Null);
            object.remove("media_asset_id");
            object.remove("content_type");
            object.insert("type".into(), kind);
            object.insert("url".into(), json!(app.storage.url(&key).await?));
        }
    }
    Ok(asset)
}
async fn serialize_post(app: &App, mut post: Value) -> Result<Value> {
    let object = post.as_object_mut().unwrap();
    let tags = object.remove("_tags").unwrap();
    let tags = tags.as_array().unwrap();
    object.insert(
        "tag_string".into(),
        json!(
            tags.iter()
                .filter_map(|t| t["name"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );
    object.insert("tag_count".into(), json!(tags.len()));
    for (category, name) in [
        (0, "general"),
        (1, "artist"),
        (3, "copyright"),
        (4, "character"),
        (5, "meta"),
    ] {
        let names = tags
            .iter()
            .filter(|t| t["category"] == category)
            .filter_map(|t| t["name"].as_str())
            .collect::<Vec<_>>();
        object.insert(format!("tag_string_{name}"), json!(names.join(" ")));
        object.insert(format!("tag_count_{name}"), json!(names.len()));
    }
    let asset = serialize_asset(app, object.remove("media_asset").unwrap()).await?;
    let variants = asset["variants"].as_array().unwrap();
    let url = |kind: &str| {
        variants
            .iter()
            .find(|v| v["type"] == kind)
            .and_then(|v| v["url"].as_str())
            .unwrap_or("")
            .to_owned()
    };
    object.insert("file_url".into(), json!(url("original")));
    object.insert("large_file_url".into(), json!(url("sample")));
    object.insert("preview_file_url".into(), json!(url("180x180")));
    object.insert("media_asset".into(), asset);
    for field in ["is_pending", "is_flagged", "is_deleted", "is_banned"] {
        object.insert(field.into(), json!(false));
    }
    for field in ["has_active_children", "has_visible_children"] {
        object.insert(field.into(), object["has_children"].clone());
    }
    for field in ["score", "up_score", "down_score", "bit_flags"] {
        object.insert(field.into(), json!(0));
    }
    for field in ["uploader_id", "approver_id"] {
        object.insert(field.into(), json!(1));
    }
    for field in [
        "last_comment_bumped_at",
        "last_noted_at",
        "last_commented_at",
        "pixiv_id",
    ] {
        object.insert(field.into(), Value::Null);
    }
    object.insert("has_large".into(), json!(true));
    Ok(post)
}
pub async fn post_by_id(app: &App, id: i64) -> Result<Value> {
    let raw: Value = sqlx::query_scalar(&format!("{POST_SELECT} WHERE p.id=$1"))
        .bind(id)
        .fetch_one(&app.db)
        .await?;
    serialize_post(app, raw).await
}
async fn show_post(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    Ok(Json(post_by_id(&app, id(&value)?).await?))
}
async fn posts(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let search = Search::parse(params.get("tags").map(String::as_str).unwrap_or(""));
    let mut query = QueryBuilder::<Postgres>::new(POST_SELECT);
    search.predicates(&mut query, &app.config.owner_login);
    let page = params.get("page").map(String::as_str).unwrap_or("1");
    if let Some((prefix, boundary)) = page
        .strip_prefix('b')
        .map(|v| (" < ", v))
        .or_else(|| page.strip_prefix('a').map(|v| (" > ", v)))
    {
        let boundary = boundary
            .parse::<i64>()
            .map_err(|_| Error::bad("invalid page cursor"))?;
        query.push(" AND p.id").push(prefix).push_bind(boundary);
    }
    if search.favorite_order {
        query.push(" ORDER BY (SELECT f.id FROM favorites f WHERE f.post_id=p.id) DESC, p.id DESC");
    } else {
        query.push(if search.ascending || page.starts_with('a') {
            " ORDER BY p.id ASC"
        } else {
            " ORDER BY p.id DESC"
        });
    }
    let limit = limit(&params, 20);
    query.push(" LIMIT ").push_bind(limit);
    if !page.starts_with(['a', 'b']) {
        query.push(" OFFSET ").push_bind(offset(&params, limit));
    }
    let records: Vec<Value> = query.build_query_scalar().fetch_all(&app.db).await?;
    let mut output = Vec::with_capacity(records.len());
    for record in records {
        output.push(serialize_post(&app, record).await?);
    }
    Ok(Json(json!(output)))
}
async fn count_posts(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT count(*) FROM posts p JOIN media_assets a ON a.id=p.media_asset_id",
    );
    Search::parse(params.get("tags").map(String::as_str).unwrap_or(""))
        .predicates(&mut query, &app.config.owner_login);
    let count: i64 = query.build_query_scalar().fetch_one(&app.db).await?;
    Ok(Json(json!({"counts":{"posts":count}})))
}

pub async fn set_tags(
    tx: &mut Transaction<'_, Postgres>,
    post_id: i64,
    tags: &BTreeSet<String>,
) -> Result<()> {
    let current: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM tags t JOIN post_tags pt ON pt.tag_id=t.id WHERE pt.post_id=$1",
    )
    .bind(post_id)
    .fetch_all(&mut **tx)
    .await?;
    let current: BTreeSet<_> = current.into_iter().collect();
    for name in current.union(tags) {
        if current.contains(name) == tags.contains(name) {
            continue;
        }
        let tag_id: i64 = sqlx::query_scalar("INSERT INTO tags(name) VALUES ($1) ON CONFLICT(name) DO UPDATE SET name=EXCLUDED.name RETURNING id").bind(name).fetch_one(&mut **tx).await?;
        if tags.contains(name) {
            sqlx::query("INSERT INTO post_tags(post_id,tag_id) VALUES ($1,$2)")
                .bind(post_id)
                .bind(tag_id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("UPDATE tags SET post_count=post_count+1, updated_at=now() WHERE id=$1")
                .bind(tag_id)
                .execute(&mut **tx)
                .await?;
        } else {
            sqlx::query("DELETE FROM post_tags WHERE post_id=$1 AND tag_id=$2")
                .bind(post_id)
                .bind(tag_id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("UPDATE tags SET post_count=post_count-1, updated_at=now() WHERE id=$1")
                .bind(tag_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}
pub async fn publish(
    tx: &mut Transaction<'_, Postgres>,
    asset_id: i64,
    source: &str,
    tags: &BTreeSet<String>,
    rating: &str,
) -> Result<(i64, bool)> {
    // Serializes publication even when two uploads resolve to the same asset.
    sqlx::query("SELECT id FROM media_assets WHERE id=$1 AND status='active' FOR UPDATE")
        .bind(asset_id)
        .fetch_one(&mut **tx)
        .await?;
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE media_asset_id=$1")
        .bind(asset_id)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok((id, true));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO posts(media_asset_id,source,rating) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(asset_id)
    .bind(source)
    .bind(rating)
    .fetch_one(&mut **tx)
    .await?;
    set_tags(tx, id, tags).await?;
    Ok((id, false))
}
async fn create_post(
    State(app): State<App>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>> {
    let params = input(&headers, &bytes)?;
    let asset_id = if let Some(value) = params
        .get("media_asset_id")
        .or_else(|| params.get("post[media_asset_id]"))
    {
        id(value)?
    } else if let Some(value) = params
        .get("upload_media_asset_id")
        .or_else(|| params.get("post[upload_media_asset_id]"))
    {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT media_asset_id FROM upload_items WHERE id=$1 AND status='completed'",
        )
        .bind(id(value)?)
        .fetch_one(&app.db)
        .await?
        .ok_or_else(|| Error::bad("asset is not ready"))?
    } else {
        return Err(Error::bad(
            "media_asset_id or upload_media_asset_id is required",
        ));
    };
    let mut tx = app.db.begin().await?;
    let tags = normalize_tags(
        params
            .get("post[tag_string]")
            .map(String::as_str)
            .unwrap_or("tagme"),
    )?;
    let rating = rating(
        params
            .get("post[rating]")
            .map(String::as_str)
            .unwrap_or("g"),
    )?;
    let (post_id, duplicate) = publish(
        &mut tx,
        asset_id,
        params.get("post[source]").map(String::as_str).unwrap_or(""),
        &tags,
        rating,
    )
    .await?;
    if !duplicate && let Some(parent) = params.get("post[parent_id]").filter(|v| !v.is_empty()) {
        sqlx::query("UPDATE posts SET parent_id=$1 WHERE id=$2")
            .bind(id(parent)?)
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(post_by_id(&app, post_id).await?))
}
pub fn edit_tags(
    current: &BTreeSet<String>,
    old: Option<&str>,
    new: &str,
) -> Result<BTreeSet<String>> {
    if old.is_some()
        && new
            .split_whitespace()
            .any(|t| t.starts_with('-') || t.starts_with("rating:"))
    {
        let mut output = current.clone();
        for token in new.split_whitespace() {
            if token.starts_with("rating:") {
                continue;
            }
            if let Some(tag) = token.strip_prefix('-') {
                output.remove(&tag.to_lowercase());
            } else {
                output.extend(normalize_tags(token)?);
            }
        }
        return normalize_tags(&output.into_iter().collect::<Vec<_>>().join(" "));
    }
    let next = normalize_tags(new)?;
    if let Some(old) = old {
        let old = normalize_tags(old)?;
        let mut output = current.clone();
        for removed in old.difference(&next) {
            output.remove(removed);
        }
        output.extend(next.difference(&old).cloned());
        Ok(output)
    } else {
        Ok(next)
    }
}
async fn edit_post(
    State(app): State<App>,
    Path(value): Path<String>,
    Query(mut params): Query<Params>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>> {
    let post_id = id(&value)?;
    params.extend(input(&headers, &bytes)?);
    let mut tx = app.db.begin().await?;
    if params.contains_key("post[parent_id]") {
        sqlx::query("SELECT pg_advisory_xact_lock(109,1)")
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("SELECT id FROM posts WHERE id=$1 FOR UPDATE")
        .bind(post_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut selected_rating = params.get("post[rating]").map(String::as_str);
    if let Some(new) = params.get("post[tag_string]") {
        let current: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN post_tags pt ON pt.tag_id=t.id WHERE pt.post_id=$1",
        )
        .bind(post_id)
        .fetch_all(&mut *tx)
        .await?;
        let tags = edit_tags(
            &current.into_iter().collect(),
            params.get("post[old_tag_string]").map(String::as_str),
            new,
        )?;
        set_tags(&mut tx, post_id, &tags).await?;
        if params.contains_key("post[old_tag_string]") {
            selected_rating = new
                .split_whitespace()
                .find_map(|v| v.strip_prefix("rating:"))
                .or(selected_rating);
        }
    }
    if let Some(value) = selected_rating {
        sqlx::query("UPDATE posts SET rating=$1 WHERE id=$2")
            .bind(rating(value)?)
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(value) = params.get("post[source]") {
        sqlx::query("UPDATE posts SET source=$1 WHERE id=$2")
            .bind(value)
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(value) = params.get("post[parent_id]") {
        let parent = if value.is_empty() {
            None
        } else {
            Some(id(value)?)
        };
        // Prevent cycles, including indirect ones.
        if let Some(parent) = parent {
            let cycle: bool = sqlx::query_scalar("WITH RECURSIVE ancestors AS (SELECT id,parent_id FROM posts WHERE id=$1 UNION SELECT p.id,p.parent_id FROM posts p JOIN ancestors a ON p.id=a.parent_id) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=$2)").bind(parent).bind(post_id).fetch_one(&mut *tx).await?;
            if cycle {
                return Err(Error::bad("parent relationship would create a cycle"));
            }
        }
        sqlx::query("UPDATE posts SET parent_id=$1 WHERE id=$2")
            .bind(parent)
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE posts SET updated_at=now() WHERE id=$1")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(post_by_id(&app, post_id).await?))
}

async fn tags(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT to_jsonb(t) || '{\"is_deprecated\":false}'::jsonb FROM tags t WHERE true",
    );
    if let Some(names) = params.get("search[name_comma]") {
        query
            .push(" AND name=ANY(")
            .push_bind(
                names
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .collect::<Vec<_>>(),
            )
            .push(")");
    } else if let Some(name) = params
        .get("search[name_matches]")
        .or_else(|| params.get("search[name]"))
    {
        query.push(" AND name LIKE ").push_bind(format!(
            "{}%",
            escape_like(&name.trim_end_matches('*').to_lowercase())
        ));
    }
    if params
        .get("search[hide_empty]")
        .is_some_and(|v| v == "true" || v == "1")
    {
        query.push(" AND post_count>0");
    }
    query
        .push(" ORDER BY post_count DESC,name LIMIT ")
        .push_bind(limit(&params, 100))
        .push(" OFFSET ")
        .push_bind(offset(&params, limit(&params, 100)));
    let rows: Vec<Value> = query.build_query_scalar().fetch_all(&app.db).await?;
    Ok(Json(json!(rows)))
}
async fn autocomplete(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let prefix = params
        .get("search[query]")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    if prefix.is_empty() {
        return Ok(Json(json!([])));
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('type','tag-word','label',name,'value',name,'antecedent',NULL,'category',category,'post_count',post_count,'source','local','has_children',false,'tag',name,'completion',name,'qualified_name',name) FROM tags WHERE name LIKE $1 ORDER BY post_count DESC,name LIMIT $2")
        .bind(format!("{}%",escape_like(&prefix.to_lowercase()))).bind(limit(&params,20)).fetch_all(&app.db).await?;
    Ok(Json(json!(rows)))
}
async fn related(Query(params): Query<Params>) -> Json<Value> {
    Json(
        json!({"query":params.get("search[query]").cloned().unwrap_or_default(),"category":null,"tags":[],"wiki_page_tags":[]}),
    )
}
async fn favorites(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    if params.get("search[user_id]").is_some_and(|v| v != "1") {
        return Ok(Json(json!([])));
    }
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT to_jsonb(f) || jsonb_build_object('user_id',1) FROM favorites f WHERE true",
    );
    if let Some(ids) = params.get("search[post_id]") {
        let ids = ids
            .split([',', ' '])
            .filter(|v| !v.is_empty())
            .map(id)
            .collect::<Result<Vec<_>>>()?;
        query.push(" AND post_id=ANY(").push_bind(ids).push(")");
    }
    query
        .push(" ORDER BY id DESC LIMIT ")
        .push_bind(limit(&params, 200))
        .push(" OFFSET ")
        .push_bind(offset(&params, limit(&params, 200)));
    let rows: Vec<Value> = query.build_query_scalar().fetch_all(&app.db).await?;
    Ok(Json(json!(rows)))
}
async fn add_favorite(
    State(app): State<App>,
    Query(query): Query<Params>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>> {
    let form = input(&headers, &bytes)?;
    let post_id = id(form
        .get("post_id")
        .or_else(|| query.get("post_id"))
        .ok_or_else(|| Error::bad("post_id is required"))?)?;
    let value: Value = sqlx::query_scalar("INSERT INTO favorites(post_id) VALUES ($1) ON CONFLICT(post_id) DO UPDATE SET post_id=EXCLUDED.post_id RETURNING to_jsonb(favorites) || jsonb_build_object('user_id',1)").bind(post_id).fetch_one(&app.db).await?;
    Ok(Json(value))
}
async fn delete_favorite(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    sqlx::query("DELETE FROM favorites WHERE post_id=$1")
        .bind(id(&value)?)
        .execute(&app.db)
        .await?;
    Ok(Json(json!({"success":true})))
}
async fn post_favorites(State(app): State<App>, Path(value): Path<String>) -> Result<Json<Value>> {
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(f) || jsonb_build_object('user_id',1,'user',jsonb_build_object('id',1,'name',$2::text)) FROM favorites f WHERE post_id=$1").bind(id(&value)?).bind(&app.config.owner_login).fetch_all(&app.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn asset_by_id(app: &App, id: i64) -> Result<Value> {
    let value: Value = sqlx::query_scalar("SELECT to_jsonb(a) || jsonb_build_object('variants',COALESCE((SELECT jsonb_agg(to_jsonb(v)) FROM media_variants v WHERE v.media_asset_id=a.id),'[]'::jsonb)) FROM media_assets a WHERE id=$1").bind(id).fetch_one(&app.db).await?;
    serialize_asset(app, value).await
}
async fn assets(State(app): State<App>, Query(params): Query<Params>) -> Result<Json<Value>> {
    let ids = if let Some(ids) = params.get("search[id]") {
        ids.split([',', ' '])
            .filter(|v| !v.is_empty())
            .map(id)
            .collect::<Result<Vec<_>>>()?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM media_assets WHERE status='active' ORDER BY id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit(&params, 100))
        .bind(offset(&params, limit(&params, 100)))
        .fetch_all(&app.db)
        .await?
    };
    let mut output = Vec::new();
    for id in ids {
        match asset_by_id(&app, id).await {
            Ok(asset) => output.push(asset),
            Err(error) if error.0 == axum::http::StatusCode::NOT_FOUND => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Json(json!(output)))
}
async fn empty() -> Json<Value> {
    Json(json!([]))
}
async fn unsupported() -> Error {
    Error(
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "this feature is not supported".into(),
    )
}
async fn wiki(Path(title): Path<String>) -> Json<Value> {
    Json(
        json!({"id":0,"title":title.trim_end_matches(".json"),"body":"","other_names":[],"is_deleted":false,"created_at":"1970-01-01T00:00:00Z","updated_at":"1970-01-01T00:00:00Z"}),
    )
}
async fn source(Query(params): Query<Params>) -> Json<Value> {
    let source = params.get("url").cloned().unwrap_or_default();
    Json(
        json!({"source":source,"page_url":source,"profile_url":"","image_url":"","artist_name":"","tags":[]}),
    )
}
async fn dtext(headers: HeaderMap, bytes: Bytes) -> Result<String> {
    Ok(crate::web::escape(
        input(&headers, &bytes)?
            .get("body")
            .map(String::as_str)
            .unwrap_or(""),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_tag_edits_preserve_unrelated_changes() {
        let current = normalize_tags("cat dog new_tag").unwrap();
        assert_eq!(
            edit_tags(&current, Some("cat dog"), "cat bird").unwrap(),
            normalize_tags("cat bird new_tag").unwrap()
        );
        assert_eq!(
            edit_tags(&current, Some("cat dog"), "-dog bird rating:q").unwrap(),
            normalize_tags("cat bird new_tag").unwrap()
        );
    }
    #[test]
    fn validates_tags_and_ratings() {
        assert!(normalize_tags("-cat").is_err());
        assert!(normalize_tags("rating:e").is_err());
        assert!(rating("safe").is_err());
    }
}
