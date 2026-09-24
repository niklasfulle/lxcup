CREATE TABLE target_telemetry_samples (
    target_id UUID NOT NULL REFERENCES targets(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (target_id, collected_at)
);
CREATE INDEX target_telemetry_samples_recent_idx ON target_telemetry_samples (target_id, collected_at DESC);
