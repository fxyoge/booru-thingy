ALTER TABLE upload_items ADD COLUMN expected_size bigint CHECK (expected_size > 0);
ALTER TABLE upload_items ADD COLUMN accepted_at timestamptz;

-- Every issued capability stays tracked until expiry plus an in-flight grace period.
CREATE TABLE upload_grants (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    upload_item_id bigint NOT NULL REFERENCES upload_items(id),
    object_key text NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX upload_grants_item ON upload_grants(upload_item_id, id DESC);
CREATE INDEX upload_grants_expiry ON upload_grants(expires_at);
