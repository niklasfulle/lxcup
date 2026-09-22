CREATE TABLE docker_workloads (
    host_container_id BIGINT NOT NULL REFERENCES containers(id) ON DELETE CASCADE,
    docker_id TEXT NOT NULL, name TEXT NOT NULL, image TEXT NOT NULL, state TEXT NOT NULL, status TEXT NOT NULL,
    management_state TEXT NOT NULL CHECK (management_state IN ('discovered', 'managed')),
    discovered_at TIMESTAMPTZ NOT NULL, PRIMARY KEY (host_container_id, docker_id)
);
