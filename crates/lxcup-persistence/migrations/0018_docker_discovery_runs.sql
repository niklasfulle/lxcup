CREATE TABLE docker_discovery_runs (
    id UUID PRIMARY KEY,
    host_container_id BIGINT NOT NULL REFERENCES containers(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NULL,
    container_count INTEGER NOT NULL DEFAULT 0 CHECK (container_count >= 0),
    error_code TEXT NULL
);

CREATE INDEX docker_discovery_runs_host_started_idx
    ON docker_discovery_runs (host_container_id, started_at DESC);

ALTER TABLE docker_workloads
    ADD COLUMN change_state TEXT NOT NULL DEFAULT 'new'
        CHECK (change_state IN ('new', 'changed', 'unchanged', 'missing'));
