CREATE TABLE package_inventory_snapshots (
    target_id UUID PRIMARY KEY REFERENCES targets(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('complete')),
    package_count INTEGER NOT NULL CHECK (package_count >= 0)
);

CREATE TABLE package_inventory_packages (
    id BIGSERIAL PRIMARY KEY,
    target_id UUID NOT NULL REFERENCES package_inventory_snapshots(target_id) ON DELETE CASCADE,
    package_name TEXT NOT NULL,
    installed_version TEXT NOT NULL,
    architecture TEXT,
    source TEXT
);

CREATE INDEX package_inventory_packages_target_name_idx
    ON package_inventory_packages (target_id, package_name);
