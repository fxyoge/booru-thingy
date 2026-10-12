-- name: AddFavorite :one
INSERT INTO favorites(post_id) VALUES (sqlc.arg(post_id)) ON CONFLICT(post_id) DO UPDATE SET post_id=EXCLUDED.post_id
RETURNING (to_jsonb(favorites) || jsonb_build_object('user_id',1))::jsonb AS payload;

-- name: DeleteFavorite :exec
DELETE FROM favorites WHERE post_id=sqlc.arg(post_id);

-- name: ListPostFavorites :many
SELECT (to_jsonb(f) || jsonb_build_object('user_id',1,'user',jsonb_build_object('id',1,'name',sqlc.arg(owner_login)::text)))::jsonb AS payload
FROM favorites f WHERE post_id=sqlc.arg(post_id);
