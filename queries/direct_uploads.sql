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
UPDATE upload_items SET staging_key=sqlc.arg(staging_key),accepted_at=now(),updated_at=now() WHERE id=sqlc.arg(id);

-- name: ExpiredUploadGrants :many
SELECT id,object_key FROM upload_grants WHERE expires_at<now()-interval '1 hour' ORDER BY id LIMIT 100;

-- name: DeleteUploadGrant :exec
DELETE FROM upload_grants WHERE id=sqlc.arg(id);

-- name: CreateUploadSnapshot :exec
INSERT INTO upload_snapshots(upload_item_id,object_key) VALUES(sqlc.arg(upload_item_id),sqlc.arg(object_key));

-- name: UnusedUploadSnapshots :many
SELECT s.id,s.object_key FROM upload_snapshots s JOIN upload_items i ON i.id=s.upload_item_id
WHERE s.created_at<now()-interval '2 hours' AND i.staging_key IS DISTINCT FROM s.object_key
ORDER BY s.id LIMIT 100;

-- name: DeleteUploadSnapshot :exec
DELETE FROM upload_snapshots WHERE id=sqlc.arg(id);
