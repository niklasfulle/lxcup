ALTER TABLE worker_heartbeats
    ADD COLUMN artifact_store_available BOOLEAN,
    ADD COLUMN artifact_store_checked_at TIMESTAMPTZ;
