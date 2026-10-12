-- Table definitions from apalis-postgres 1.0.0, for sqlc's schema analysis.
-- Source: https://github.com/apalis-dev/apalis-postgres/tree/v1.0.0/migrations
CREATE SCHEMA apalis;
CREATE EXTENSION hstore;
CREATE TABLE apalis.workers (
    id text PRIMARY KEY,
    worker_type text NOT NULL,
    storage_name text NOT NULL,
    layers text NOT NULL DEFAULT '',
    last_seen timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz
);
CREATE TABLE apalis.jobs (
    id text PRIMARY KEY,
    job_type text NOT NULL,
    job bytea NOT NULL,
    status text NOT NULL DEFAULT 'Pending',
    attempts integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 25,
    run_at timestamptz NOT NULL DEFAULT now(),
    last_result jsonb,
    lock_at timestamptz,
    lock_by text REFERENCES apalis.workers(id),
    done_at timestamptz,
    priority integer DEFAULT 0,
    idempotency_key text,
    metadata hstore
);
