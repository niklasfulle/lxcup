CREATE TABLE target_docker_telemetry_samples (
    target_id UUID NOT NULL REFERENCES targets(id) ON DELETE CASCADE,
    container_id TEXT NOT NULL,
    collected_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (target_id, container_id, collected_at)
);

CREATE INDEX target_docker_telemetry_samples_recent_idx
    ON target_docker_telemetry_samples (target_id, collected_at DESC);
