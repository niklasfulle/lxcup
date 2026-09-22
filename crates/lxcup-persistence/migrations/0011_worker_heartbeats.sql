CREATE TABLE worker_heartbeats (
    worker_name TEXT PRIMARY KEY,
    last_seen_at TIMESTAMPTZ NOT NULL
);

-- This is the database-level exclusion rule behind per-target worker
-- exclusivity. Queued work may accumulate, but only one job may be claimed
-- into an executing state for a target at a time.
CREATE UNIQUE INDEX ansible_jobs_one_active_target_idx
    ON ansible_jobs (target)
    WHERE status IN ('checking', 'planned', 'applying', 'reconcile_required');
