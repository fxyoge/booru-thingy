-- name: GetSession :one
SELECT csrf_token FROM web_sessions WHERE token_hash=sqlc.arg(token_hash) AND expires_at>now();

-- name: CreateSession :exec
INSERT INTO web_sessions(token_hash,csrf_token,expires_at)
VALUES (sqlc.arg(token_hash),sqlc.arg(csrf_token),now()+interval '7 days');

-- name: DeleteSession :exec
DELETE FROM web_sessions WHERE token_hash=sqlc.arg(token_hash);

-- name: DeleteExpiredSessions :exec
DELETE FROM web_sessions WHERE expires_at<now();

-- name: CheckDatabaseReady :one
SELECT id FROM user_settings WHERE id=1;
