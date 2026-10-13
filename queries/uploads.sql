-- name: CreateUpload :one
INSERT INTO uploads DEFAULT VALUES RETURNING id;

-- name: TouchUploadItem :exec
UPDATE upload_items SET updated_at=now() WHERE id=sqlc.arg(id);

-- name: UpdateUploadMetadata :exec
UPDATE uploads SET source=sqlc.arg(source),tag_string=sqlc.arg(tag_string),rating=sqlc.arg(rating),
referer_url=sqlc.arg(referer_url),updated_at=now() WHERE id=sqlc.arg(id);

-- name: CreateSourceItem :one
INSERT INTO upload_items(upload_id,source_url,filename)
VALUES (sqlc.arg(upload_id),sqlc.arg(source_url),sqlc.arg(filename)) RETURNING id;

-- name: GetUpload :one
SELECT to_jsonb(u) AS payload FROM uploads u WHERE id=sqlc.arg(id);

-- name: ListUploadItems :many
SELECT i.id,i.upload_id,i.filename,i.source_url,i.media_asset_id,i.post_id,i.duplicate,
    s.status,s.error,i.created_at::text AS created,i.updated_at::text AS updated
FROM upload_items i JOIN upload_item_states s ON s.id=i.id
WHERE i.upload_id=ANY(sqlc.arg(upload_ids)::bigint[]) ORDER BY i.upload_id,i.id;

-- name: RecentUploads :many
SELECT to_jsonb(u) AS payload FROM uploads u ORDER BY id DESC LIMIT sqlc.arg(result_limit)::bigint;

-- name: ListUploads :many
SELECT to_jsonb(u) AS payload FROM uploads u
WHERE (sqlc.narg(is_posted)::boolean IS NULL OR
       EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.post_id IS NOT NULL)=sqlc.narg(is_posted)::boolean)
AND CASE
    WHEN sqlc.narg(status)::text IS NULL THEN true
    WHEN sqlc.narg(status)::text IN ('pending','processing') THEN
        EXISTS(SELECT 1 FROM upload_items i JOIN upload_item_states s ON s.id=i.id
               WHERE i.upload_id=u.id AND s.status IN ('receiving','pending','processing'))
    WHEN sqlc.narg(status)::text='completed' THEN
        EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id)
        AND NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id AND i.post_id IS NULL)
    WHEN sqlc.narg(status)::text='error' THEN
        (NOT EXISTS(SELECT 1 FROM upload_items i WHERE i.upload_id=u.id)
         OR EXISTS(SELECT 1 FROM upload_items i JOIN upload_item_states s ON s.id=i.id WHERE i.upload_id=u.id AND s.status='error'))
        AND NOT EXISTS(SELECT 1 FROM upload_items i JOIN upload_item_states s ON s.id=i.id
                       WHERE i.upload_id=u.id AND s.status IN ('receiving','pending','processing'))
    ELSE false END
ORDER BY CASE WHEN sqlc.arg(ascending)::boolean THEN u.id END ASC,
         CASE WHEN NOT sqlc.arg(ascending)::boolean THEN u.id END DESC
LIMIT sqlc.arg(result_limit)::bigint OFFSET sqlc.arg(result_offset)::bigint;

-- name: LockRetryUploadItems :many
SELECT i.id FROM upload_items i JOIN upload_item_states s ON s.id=i.id
WHERE i.upload_id=sqlc.arg(upload_id) AND s.status='error'
AND i.expected_size IS NULL AND i.source_url <> '' FOR UPDATE OF i;

-- name: StartUploadExecution :exec
UPDATE upload_items SET task_id=sqlc.arg(task_id),updated_at=now()
WHERE id=sqlc.arg(id);

-- name: GetUploadItemState :one
SELECT status,error FROM upload_item_states WHERE id=sqlc.arg(id);

-- name: GetProcessingItem :one
SELECT source_url,input_key,input_etag,expected_size,task_id,post_id FROM upload_items WHERE id=sqlc.arg(id);

-- name: LockProcessingItem :one
SELECT i.upload_id,i.post_id,i.task_id,u.source,u.tag_string,u.rating FROM upload_items i JOIN uploads u ON u.id=i.upload_id
WHERE i.id=sqlc.arg(id) FOR UPDATE OF i;

-- name: CompleteUploadItem :exec
UPDATE upload_items SET media_asset_id=sqlc.arg(media_asset_id),post_id=sqlc.arg(post_id),
duplicate=sqlc.arg(duplicate),updated_at=now() WHERE id=sqlc.arg(id);

-- name: TouchUpload :exec
UPDATE uploads SET updated_at=now() WHERE id=sqlc.arg(id);

-- name: ListInputCleanupCandidates :many
SELECT i.id FROM upload_items i JOIN upload_item_states s ON s.id=i.id
WHERE i.input_key IS NOT NULL AND s.status IN ('completed','error') LIMIT 100;

-- name: LockInputCleanupItem :one
SELECT i.input_key FROM upload_items i JOIN upload_item_states s ON s.id=i.id
WHERE i.id=sqlc.arg(id) AND s.status IN ('completed','error') FOR UPDATE OF i;

-- name: ClearInput :exec
UPDATE upload_items SET input_key=NULL,input_etag=NULL WHERE id=sqlc.arg(id);

-- name: GetCompletedItemAsset :one
SELECT media_asset_id FROM upload_items WHERE id=sqlc.arg(id) AND post_id IS NOT NULL;
