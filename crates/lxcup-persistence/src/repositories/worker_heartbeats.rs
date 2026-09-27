use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{Database, RepositoryError, WorkerHeartbeatRepository};

#[derive(Clone, Debug)]
pub struct WorkerHeartbeatStatus {
    pub last_seen_at: DateTime<Utc>,
    pub artifact_store_available: Option<bool>,
    pub artifact_store_checked_at: Option<DateTime<Utc>>,
}

impl WorkerHeartbeatRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    /// Records that this worker has completed another poll cycle.
    pub async fn record(
        &self,
        worker_name: &str,
        artifact_store_available: bool,
        artifact_store_checked_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO worker_heartbeats (worker_name, last_seen_at, artifact_store_available, artifact_store_checked_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (worker_name) DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at, artifact_store_available = EXCLUDED.artifact_store_available, artifact_store_checked_at = EXCLUDED.artifact_store_checked_at",
        )
        .bind(worker_name)
        .bind(Utc::now())
        .bind(artifact_store_available)
        .bind(artifact_store_checked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns the newest heartbeat that is still inside the requested window.
    pub async fn latest_since(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, RepositoryError> {
        let row = sqlx::query(
            "SELECT MAX(last_seen_at) AS last_seen_at FROM worker_heartbeats WHERE last_seen_at >= $1",
        )
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await?;
        row.try_get("last_seen_at").map_err(Into::into)
    }

    pub async fn latest(&self) -> Result<Option<DateTime<Utc>>, RepositoryError> {
        let row = sqlx::query("SELECT MAX(last_seen_at) AS last_seen_at FROM worker_heartbeats")
            .fetch_one(&self.pool)
            .await?;
        row.try_get("last_seen_at").map_err(Into::into)
    }

    pub async fn latest_status(&self) -> Result<Option<WorkerHeartbeatStatus>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT last_seen_at, artifact_store_available, artifact_store_checked_at FROM worker_heartbeats ORDER BY last_seen_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(WorkerHeartbeatStatus {
            last_seen_at: row.try_get("last_seen_at")?,
            artifact_store_available: row.try_get("artifact_store_available")?,
            artifact_store_checked_at: row.try_get("artifact_store_checked_at")?,
        }))
    }
}
