CREATE TABLE nodes (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    address TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('unknown', 'connected', 'disconnected')),
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE containers (
    id BIGINT PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES nodes (id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    operating_system TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'stopped', 'unknown')),
    management_state TEXT NOT NULL CHECK (
        management_state IN ('discovered', 'managed', 'ignored', 'disabled')
    ),
    discovered_at TIMESTAMPTZ NOT NULL,
    UNIQUE (node_id, name)
);

CREATE INDEX containers_node_id_idx ON containers (node_id);

CREATE TABLE scans (
    id UUID PRIMARY KEY,
    container_id BIGINT NOT NULL REFERENCES containers (id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    created_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ
);

CREATE INDEX scans_container_id_created_at_idx ON scans (container_id, created_at DESC);

CREATE TABLE scan_updates (
    scan_id UUID NOT NULL REFERENCES scans (id) ON DELETE CASCADE,
    package_name TEXT NOT NULL,
    installed_version TEXT NOT NULL,
    candidate_version TEXT NOT NULL,
    classification TEXT NOT NULL CHECK (classification IN ('security', 'normal', 'unknown')),
    held BOOLEAN NOT NULL,
    PRIMARY KEY (scan_id, package_name)
);

CREATE TABLE update_plans (
    id UUID PRIMARY KEY,
    container_id BIGINT NOT NULL REFERENCES containers (id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (
        status IN ('draft', 'blocked', 'ready', 'confirmed', 'invalidated')
    ),
    created_at TIMESTAMPTZ NOT NULL,
    plan_hash TEXT
);

CREATE INDEX update_plans_container_id_created_at_idx
    ON update_plans (container_id, created_at DESC);

CREATE TABLE update_plan_requested_packages (
    plan_id UUID NOT NULL REFERENCES update_plans (id) ON DELETE RESTRICT,
    package_name TEXT NOT NULL,
    PRIMARY KEY (plan_id, package_name)
);

CREATE TABLE update_plan_changes (
    plan_id UUID NOT NULL REFERENCES update_plans (id) ON DELETE RESTRICT,
    package_name TEXT NOT NULL,
    from_version TEXT,
    to_version TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('install', 'upgrade', 'remove', 'downgrade')),
    PRIMARY KEY (plan_id, package_name, kind)
);

CREATE TABLE executions (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES update_plans (id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'succeeded', 'failed', 'aborted', 'unknown')
    ),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX executions_plan_id_idx ON executions (plan_id);

CREATE TABLE execution_events (
    id UUID PRIMARY KEY,
    execution_id UUID NOT NULL REFERENCES executions (id) ON DELETE RESTRICT,
    sequence BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    message TEXT,
    fields JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE (execution_id, sequence)
);

CREATE INDEX execution_events_execution_id_sequence_idx
    ON execution_events (execution_id, sequence);

CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    node_id UUID REFERENCES nodes (id) ON DELETE RESTRICT,
    container_id BIGINT REFERENCES containers (id) ON DELETE RESTRICT,
    plan_id UUID REFERENCES update_plans (id) ON DELETE RESTRICT,
    execution_id UUID REFERENCES executions (id) ON DELETE RESTRICT,
    event_type TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX audit_events_created_at_idx ON audit_events (created_at DESC);
CREATE INDEX audit_events_container_id_created_at_idx
    ON audit_events (container_id, created_at DESC);
