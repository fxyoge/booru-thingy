-- name: ListPostTags :many
SELECT t.name FROM tags t JOIN post_tags pt ON pt.tag_id=t.id WHERE pt.post_id=sqlc.arg(post_id);

-- name: UpsertTag :one
INSERT INTO tags(name) VALUES (sqlc.arg(name)) ON CONFLICT(name) DO UPDATE SET name=EXCLUDED.name RETURNING id;

-- name: AddPostTag :exec
INSERT INTO post_tags(post_id,tag_id) VALUES (sqlc.arg(post_id),sqlc.arg(tag_id));

-- name: IncrementTagCount :exec
UPDATE tags SET post_count=post_count+1, updated_at=now() WHERE id=sqlc.arg(id);

-- name: RemovePostTag :exec
DELETE FROM post_tags WHERE post_id=sqlc.arg(post_id) AND tag_id=sqlc.arg(tag_id);

-- name: DecrementTagCount :exec
UPDATE tags SET post_count=post_count-1, updated_at=now() WHERE id=sqlc.arg(id);

-- name: AutocompleteTags :many
SELECT jsonb_build_object('type','tag-word','label',name,'value',name,'antecedent',NULL,'category',category,
'post_count',post_count,'source','local','has_children',false,'tag',name,'completion',name,'qualified_name',name) AS payload
FROM tags WHERE name LIKE sqlc.arg(prefix) ORDER BY post_count DESC,name LIMIT sqlc.arg(result_limit)::bigint;
