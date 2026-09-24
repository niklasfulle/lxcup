use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{Database, RepositoryError, WorkerHeartbeatRepository};

impl WorkerHeartbeatRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    /// Records that this worker has completed another poll cycle.
    pub async fn record(&self, worker_name: &str) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO worker_heartbeats (worker_name, last_seen_at) VALUES ($1, $2) \
             ON CONFLICT (worker_name) DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at",
        )
        .bind(worker_name)
        .bind(Utc::now())
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
}
