ALTER TABLE target_docker_inventory
    ADD COLUMN events JSONB NOT NULL DEFAULT '[]'::jsonb
    CHECK (jsonb_typeof(events) = 'array');
