CREATE TABLE auth_users (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    disabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE auth_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX auth_sessions_user_id_idx ON auth_sessions(user_id);
CREATE INDEX auth_sessions_expiration_idx ON auth_sessions(expires_at) WHERE revoked_at IS NULL;

CREATE TABLE user_audit_events (
    id UUID PRIMARY KEY,
    actor_user_id UUID REFERENCES auth_users(id) ON DELETE SET NULL,
    actor_username TEXT NOT NULL,
    actor_role TEXT NOT NULL CHECK (actor_role IN ('admin', 'user')),
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    request_id UUID,
    status_code SMALLINT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX user_audit_events_created_at_idx ON user_audit_events(created_at DESC);
CREATE INDEX user_audit_events_actor_idx ON user_audit_events(actor_user_id, created_at DESC);
