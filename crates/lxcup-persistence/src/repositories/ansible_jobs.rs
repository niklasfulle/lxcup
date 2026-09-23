use super::*;

impl AnsibleJobRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, job: &AnsibleJob) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(job).map_err(RepositoryError::Serialization)?;
        let target = serde_json::to_value(job.target).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO ansible_jobs (id, target, idempotency_key, status, payload, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(job.id.as_uuid())
        .bind(target)
        .bind(&job.idempotency_key)
        .bind(ansible_status_to_db(job.status))
        .bind(payload)
        .bind(job.created_at)
        .bind(job.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, job: &AnsibleJob) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(job).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "UPDATE ansible_jobs SET status = $1, payload = $2, updated_at = $3 WHERE id = $4",
        )
        .bind(ansible_status_to_db(job.status))
        .bind(payload)
        .bind(job.updated_at)
        .bind(job.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(
        &self,
        id: lxcup_core::AnsibleJobId,
    ) -> Result<Option<AnsibleJob>, RepositoryError> {
        let row = sqlx::query("SELECT payload FROM ansible_jobs WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                RepositoryError::InvalidValue {
                    field: "ansible job payload",
                }
            })
        })
        .transpose()
    }

    /// Claims exactly one queued job. `SKIP LOCKED` lets several worker
    /// processes poll concurrently without ever executing the same job.
    pub async fn claim_next_queued(&self) -> Result<Option<AnsibleJob>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT payload FROM ansible_jobs WHERE status = 'queued' ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(None);
        };
        let mut job: AnsibleJob =
            serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                RepositoryError::InvalidValue {
                    field: "ansible job payload",
                }
            })?;
        job.transition_to(AnsibleJobStatus::Checking).map_err(|_| {
            RepositoryError::InvalidValue {
                field: "ansible job status",
            }
        })?;
        let payload = serde_json::to_value(&job).map_err(RepositoryError::Serialization)?;
        let update = sqlx::query("UPDATE ansible_jobs SET status = 'checking', payload = $1, updated_at = $2 WHERE id = $3")
            .bind(payload)
            .bind(job.updated_at)
            .bind(job.id.as_uuid())
            .execute(&mut *transaction)
            .await;
        if let Err(error) = update {
            if error
                .as_database_error()
                .and_then(|database| database.code())
                .as_deref()
                == Some("23505")
            {
                transaction.rollback().await?;
                return Ok(None);
            }
            return Err(error.into());
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM ansible_job_events WHERE job_id = $1",
        )
        .bind(job.id.as_uuid())
        .fetch_one(&mut *transaction)
        .await?;
        let event = JobEvent {
            sequence: sequence as u64,
            job_id: job.id,
            event: lxcup_ansible::JobEventKind::StatusChanged {
                status: AnsibleJobStatus::Checking,
            },
            created_at: Utc::now(),
        };
        sqlx::query("INSERT INTO ansible_job_events (job_id, sequence, payload, created_at) VALUES ($1, $2, $3, $4)")
            .bind(event.job_id.as_uuid())
            .bind(sequence)
            .bind(serde_json::to_value(event).map_err(RepositoryError::Serialization)?)
            .bind(Utc::now())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(job))
    }

    pub async fn list(&self) -> Result<Vec<AnsibleJob>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM ansible_jobs ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "ansible job payload",
                    }
                })
            })
            .collect()
    }

    pub async fn find_by_idempotency_key(
        &self,
        target: lxcup_core::ResourceTarget,
        key: &str,
    ) -> Result<Option<AnsibleJob>, RepositoryError> {
        let target = serde_json::to_value(target).map_err(RepositoryError::Serialization)?;
        let row = sqlx::query(
            "SELECT payload FROM ansible_jobs WHERE target = $1 AND idempotency_key = $2",
        )
        .bind(target)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                RepositoryError::InvalidValue {
                    field: "ansible job payload",
                }
            })
        })
        .transpose()
    }

    pub async fn has_active_target(
        &self,
        target: lxcup_core::ResourceTarget,
    ) -> Result<bool, RepositoryError> {
        let target = serde_json::to_value(target).map_err(RepositoryError::Serialization)?;
        let rows = sqlx::query("SELECT status FROM ansible_jobs WHERE target = $1")
            .bind(target)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().any(|row| {
            row.try_get::<String, _>("status")
                .is_ok_and(|status| is_active_ansible_job_status(&status))
        }))
    }

    pub async fn append_event(&self, event: &JobEvent) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(event).map_err(RepositoryError::Serialization)?;
        let sequence =
            i64::try_from(event.sequence).map_err(|_| RepositoryError::InvalidValue {
                field: "ansible event sequence",
            })?;
        sqlx::query(
            "INSERT INTO ansible_job_events (job_id, sequence, payload, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(event.job_id.as_uuid())
        .bind(sequence)
        .bind(payload)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn events(
        &self,
        id: lxcup_core::AnsibleJobId,
    ) -> Result<Vec<JobEvent>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT payload FROM ansible_job_events WHERE job_id = $1 ORDER BY sequence",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "ansible job event payload",
                    }
                })
            })
            .collect()
    }
}
