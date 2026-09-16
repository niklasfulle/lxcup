ALTER TABLE nodes
    ADD COLUMN proxmox_version TEXT,
    ADD COLUMN capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN last_checked_at TIMESTAMPTZ,
    ADD COLUMN last_check_error TEXT;
