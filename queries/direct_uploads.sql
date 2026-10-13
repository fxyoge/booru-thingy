-- name: CreateDirectItem :one
INSERT INTO upload_items(upload_id,filename,expected_size)
VALUES(sqlc.arg(upload_id),sqlc.arg(filename),sqlc.arg(expected_size)) RETURNING id;

-- name: LockDirectItem :one
SELECT i.id,s.status,i.expected_size,i.accepted_at FROM upload_items i JOIN upload_item_states s ON s.id=i.id
WHERE i.id=sqlc.arg(id) AND i.upload_id=sqlc.arg(upload_id) AND i.expected_size IS NOT NULL FOR UPDATE OF i;

-- name: CreateUploadGrant :one
INSERT INTO upload_grants(upload_item_id,object_key,expires_at)
VALUES(sqlc.arg(upload_item_id),sqlc.arg(object_key),sqlc.arg(expires_at)) RETURNING id;

-- name: CountUploadGrants :one
SELECT count(*)::bigint AS count FROM upload_grants WHERE upload_item_id=sqlc.arg(upload_item_id);

-- name: LatestUploadGrant :one
SELECT object_key,expires_at FROM upload_grants WHERE upload_item_id=sqlc.arg(upload_item_id) ORDER BY id DESC LIMIT 1;

-- name: AcceptDirectItem :exec
UPDATE upload_items SET input_key=sqlc.arg(input_key),input_etag=sqlc.arg(input_etag),accepted_at=now(),updated_at=now() WHERE id=sqlc.arg(id);

-- name: ExpiredUploadGrants :many
SELECT g.id,g.object_key FROM upload_grants g JOIN upload_items i ON i.id=g.upload_item_id
WHERE g.expires_at<now()-interval '1 hour' AND i.input_key IS DISTINCT FROM g.object_key
ORDER BY g.id LIMIT 100;

-- name: DeleteUploadGrant :exec
DELETE FROM upload_grants WHERE id=sqlc.arg(id);
