-- Read-only queue adapter: execution state exists only in Apalis. Publication
-- wins over queue state because a process can die between commit and acknowledgement.
CREATE VIEW upload_item_states AS
SELECT i.id,
    CASE WHEN i.post_id IS NOT NULL THEN 'completed'
         WHEN i.task_id IS NULL THEN
             CASE WHEN i.updated_at < now()-interval '24 hours' THEN 'error' ELSE 'receiving' END
         WHEN j.status IN ('Queued','Running') THEN 'processing'
         WHEN j.status='Pending' OR (j.status='Failed' AND j.attempts<j.max_attempts) THEN 'pending'
         ELSE 'error' END::text AS status,
    CASE WHEN i.post_id IS NOT NULL THEN ''
         WHEN i.task_id IS NULL THEN
             CASE WHEN i.updated_at < now()-interval '24 hours' THEN 'Upload interrupted' ELSE '' END
         WHEN j.status='Killed' OR (j.status='Failed' AND j.attempts>=j.max_attempts) THEN
             COALESCE(j.last_result->>'Err','Upload failed')
         WHEN j.id IS NULL THEN 'Upload task is missing'
         WHEN j.status='Done' THEN 'Upload task finished without publication'
         ELSE '' END::text AS error
FROM upload_items i LEFT JOIN apalis.jobs j ON j.id=i.task_id;
