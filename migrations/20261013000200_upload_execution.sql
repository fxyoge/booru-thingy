-- All steps carry the same run_id in their
-- JSON payload, including duplicate successors created by at-least-once delivery.
CREATE FUNCTION upload_task_run(data bytea) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN convert_from(data,'UTF8')::jsonb->>'run_id';

CREATE INDEX upload_execution_tasks ON apalis.jobs (upload_task_run(job))
    WHERE job_type='uploads-v1';

CREATE VIEW upload_item_states AS
SELECT i.id,
    CASE WHEN i.status IN ('receiving','completed','error') THEN i.status
         WHEN execution.running THEN 'processing'
         WHEN execution.active THEN 'pending'
         WHEN execution.failed THEN 'error'
         ELSE 'pending' END::text AS status,
    COALESCE(CASE WHEN i.status IN ('completed','receiving') THEN NULL
         WHEN i.status='error' THEN i.error
         WHEN NOT execution.active AND execution.failed THEN execution.error
         ELSE i.error END,'')::text AS error
FROM upload_items i LEFT JOIN LATERAL (
    SELECT bool_or(j.status IN ('Pending','Queued','Running') OR
                   (j.status='Failed' AND j.attempts<j.max_attempts)) AS active,
           bool_or(j.status IN ('Queued','Running')) AS running,
           bool_or(j.status='Killed' OR (j.status='Failed' AND j.attempts>=j.max_attempts)) AS failed,
           max(j.last_result->>'Err') AS error
    FROM apalis.jobs j
    WHERE j.job_type='uploads-v1' AND upload_task_run(j.job)=i.run_id
) execution ON true;
