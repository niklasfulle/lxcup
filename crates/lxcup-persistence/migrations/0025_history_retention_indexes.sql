CREATE INDEX target_telemetry_samples_retention_idx
    ON target_telemetry_samples (collected_at);

CREATE INDEX target_docker_telemetry_samples_retention_idx
    ON target_docker_telemetry_samples (collected_at);
