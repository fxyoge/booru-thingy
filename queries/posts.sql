-- name: GetOwner :one
SELECT s.*, (SELECT count(*) FROM posts) AS uploads, (SELECT count(*) FROM favorites) AS favorites
FROM user_settings s WHERE id=1;

-- name: GetPost :one
SELECT (to_jsonb(p) || jsonb_build_object('md5',a.md5,'file_ext',a.file_ext,'file_size',a.file_size,
'image_width',a.image_width,'image_height',a.image_height,'fav_count',(SELECT count(*) FROM favorites f WHERE f.post_id=p.id),
'has_children',EXISTS(SELECT 1 FROM posts child WHERE child.parent_id=p.id),
'_tags',COALESCE((SELECT jsonb_agg(jsonb_build_object('name',t.name,'category',t.category) ORDER BY t.name)
FROM post_tags pt JOIN tags t ON t.id=pt.tag_id WHERE pt.post_id=p.id),'[]'::jsonb),
'media_asset',to_jsonb(a) || jsonb_build_object('variants',COALESCE((SELECT jsonb_agg(to_jsonb(v) ORDER BY v.id)
FROM media_variants v WHERE v.media_asset_id=a.id),'[]'::jsonb))))::jsonb AS payload
FROM posts p JOIN media_assets a ON a.id=p.media_asset_id WHERE p.id=sqlc.arg(id);

-- name: LockActiveAsset :one
SELECT id FROM media_assets WHERE id=sqlc.arg(id) AND status='active' FOR UPDATE;

-- name: GetPostByAsset :one
SELECT id FROM posts WHERE media_asset_id=sqlc.arg(media_asset_id);

-- name: CreatePost :one
INSERT INTO posts(media_asset_id,source,rating) VALUES (sqlc.arg(media_asset_id),sqlc.arg(source),sqlc.arg(rating)) RETURNING id;

-- name: SetPostParent :exec
UPDATE posts SET parent_id=sqlc.narg(parent_id) WHERE id=sqlc.arg(id);

-- name: LockParentRelationships :exec
SELECT pg_advisory_xact_lock(109,1);

-- name: LockPost :one
SELECT id FROM posts WHERE id=sqlc.arg(id) FOR UPDATE;

-- name: SetPostRating :exec
UPDATE posts SET rating=sqlc.arg(rating) WHERE id=sqlc.arg(id);

-- name: SetPostSource :exec
UPDATE posts SET source=sqlc.arg(source) WHERE id=sqlc.arg(id);

-- name: WouldCreateParentCycle :one
WITH RECURSIVE ancestors AS (SELECT posts.id AS ancestor_id,posts.parent_id FROM posts WHERE posts.id=sqlc.arg(parent_id)
UNION SELECT p.id,p.parent_id FROM posts p JOIN ancestors a ON p.id=a.parent_id)
SELECT EXISTS(SELECT 1 FROM ancestors WHERE ancestor_id=sqlc.arg(post_id)::bigint) AS cycle;

-- name: TouchPost :exec
UPDATE posts SET updated_at=now() WHERE id=sqlc.arg(id);
