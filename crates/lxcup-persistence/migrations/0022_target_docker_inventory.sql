CREATE TABLE target_docker_inventory (
    target_id UUID PRIMARY KEY REFERENCES targets(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    containers JSONB NOT NULL
);
