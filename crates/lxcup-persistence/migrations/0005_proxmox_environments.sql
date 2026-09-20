CREATE TABLE proxmox_environments (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    endpoint TEXT NOT NULL,
    api_secret_ref UUID NOT NULL,
    ca_secret_ref UUID,
    status TEXT NOT NULL CHECK (status IN ('configured', 'connected', 'failed', 'disabled')),
    last_checked_at TIMESTAMPTZ,
    last_check_error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
