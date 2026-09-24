use super::{Database, RepositoryError};
use chrono::Utc;
use lxcup_agent::{AgentHeartbeat, SystemTelemetrySample};
use lxcup_core::TargetId;
use sqlx::Row;

impl super::TelemetryRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn append_heartbeat(
        &self,
        heartbeat: &AgentHeartbeat,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();
        for sample in &heartbeat.telemetry.samples {
            // Accept only the bounded window emitted by an agent. Future
            // timestamps and stale replays must never pollute the time series.
            if sample.collected_at > now + chrono::Duration::seconds(5)
                || sample.collected_at < now - chrono::Duration::seconds(30)
            {
                continue;
            }
            sqlx::query("INSERT INTO target_telemetry_samples (target_id, collected_at, payload) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(heartbeat.target_id).bind(sample.collected_at).bind(serde_json::to_value(sample).map_err(RepositoryError::Serialization)?).execute(&mut *tx).await?;
        }
        sqlx::query(
            "DELETE FROM target_telemetry_samples WHERE target_id=$1 AND collected_at < $2",
        )
        .bind(heartbeat.target_id)
        .bind(now - chrono::Duration::seconds(30))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_recent(
        &self,
        target_id: TargetId,
    ) -> Result<Vec<SystemTelemetrySample>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM target_telemetry_samples WHERE target_id=$1 AND collected_at >= $2 ORDER BY collected_at")
            .bind(target_id.as_uuid()).bind(Utc::now() - chrono::Duration::seconds(30)).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "telemetry sample",
                    }
                })
            })
            .collect()
    }
}
