DROP INDEX ansible_jobs_one_active_target_idx;

CREATE UNIQUE INDEX ansible_jobs_one_active_target_idx
    ON ansible_jobs (target)
    WHERE status IN ('checking', 'planned', 'applying');
