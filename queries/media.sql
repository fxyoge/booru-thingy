-- name: GetAssetByHash :one
SELECT id,status FROM media_assets WHERE sha256=sqlc.arg(sha256);

-- name: UpsertMediaAsset :one
INSERT INTO media_assets(sha256,md5,file_ext,content_type,file_size,image_width,image_height,duration,original_key,status)
VALUES (sqlc.arg(sha256),sqlc.arg(md5),sqlc.arg(file_ext),sqlc.arg(content_type),sqlc.arg(file_size),
sqlc.arg(image_width),sqlc.arg(image_height),sqlc.narg(duration),sqlc.arg(original_key),'active')
ON CONFLICT(sha256) DO UPDATE SET status='active',updated_at=now() RETURNING id;

-- name: InsertMediaVariants :exec
INSERT INTO media_variants(media_asset_id,kind,object_key,file_ext,content_type,width,height,file_size)
SELECT sqlc.arg(media_asset_id),v.kind,v.key,v.ext,v.content_type,v.width,v.height,v.size
FROM jsonb_to_recordset(sqlc.arg(variants)::jsonb)
AS v(kind text,key text,ext text,content_type text,width integer,height integer,size bigint)
ON CONFLICT(media_asset_id,kind) DO NOTHING;

-- name: GetMediaAsset :one
SELECT (to_jsonb(a) || jsonb_build_object('variants',COALESCE((SELECT jsonb_agg(to_jsonb(v))
FROM media_variants v WHERE v.media_asset_id=a.id),'[]'::jsonb)))::jsonb AS payload
FROM media_assets a WHERE a.id=sqlc.arg(id);

-- name: GetMediaAssets :many
SELECT a.id,(to_jsonb(a) || jsonb_build_object('variants',COALESCE((SELECT jsonb_agg(to_jsonb(v))
FROM media_variants v WHERE v.media_asset_id=a.id),'[]'::jsonb)))::jsonb AS payload
FROM media_assets a WHERE a.id=ANY(sqlc.arg(ids)::bigint[]);

-- name: ListActiveAssetIds :many
SELECT id FROM media_assets WHERE status='active' ORDER BY id DESC
LIMIT sqlc.arg(result_limit)::bigint OFFSET sqlc.arg(result_offset)::bigint;
