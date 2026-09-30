ALTER TABLE ansible_jobs
    ADD COLUMN target_id UUID REFERENCES targets (id) ON DELETE CASCADE;

UPDATE ansible_jobs
SET target_id = (target ->> 'target')::UUID
WHERE target ->> 'target' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$';

CREATE INDEX ansible_jobs_target_id_idx ON ansible_jobs (target_id);
