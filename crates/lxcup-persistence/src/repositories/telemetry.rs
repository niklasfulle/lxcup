use super::{Database, RepositoryError};
use chrono::Utc;
use lxcup_agent::{AgentHeartbeat, DockerTelemetrySample, SystemTelemetrySample, TelemetryBuffer};
use lxcup_core::TargetId;
use sqlx::Row;

const RETENTION_DELETE_BATCH_SIZE: i64 = 5_000;

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
                || sample.collected_at
                    < now - chrono::Duration::seconds(TelemetryBuffer::WINDOW_SECONDS)
            {
                continue;
            }
            let overlap_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM target_telemetry_samples WHERE target_id=$1 AND collected_at BETWEEN $2 - INTERVAL '1 second' AND $2 + INTERVAL '1 second')",
            )
            .bind(heartbeat.target_id)
            .bind(sample.collected_at)
            .fetch_one(&mut *tx)
            .await?;
            if overlap_exists {
                continue;
            }
            sqlx::query("INSERT INTO target_telemetry_samples (target_id, collected_at, payload) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(heartbeat.target_id).bind(sample.collected_at).bind(serde_json::to_value(sample).map_err(RepositoryError::Serialization)?).execute(&mut *tx).await?;
        }
        for sample in heartbeat.docker_telemetry.samples.iter().take(8_000) {
            if !sample
                .container_id
                .chars()
                .all(|character| character.is_ascii_hexdigit())
                || !(12..=64).contains(&sample.container_id.len())
                || sample.collected_at > now + chrono::Duration::seconds(5)
                || sample.collected_at
                    < now - chrono::Duration::seconds(TelemetryBuffer::WINDOW_SECONDS)
                || sample
                    .memory_basis_points
                    .is_some_and(|value| value > 10_000)
            {
                continue;
            }
            sqlx::query("INSERT INTO target_docker_telemetry_samples (target_id, container_id, collected_at, payload) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                .bind(heartbeat.target_id)
                .bind(sample.container_id.to_ascii_lowercase())
                .bind(sample.collected_at)
                .bind(serde_json::to_value(sample).map_err(RepositoryError::Serialization)?)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn prune_expired(&self) -> Result<u64, RepositoryError> {
        let cutoff = Utc::now()
            - chrono::Duration::seconds(lxcup_core::telemetry::PERSISTED_RETENTION_SECONDS);
        let system = sqlx::query(
            "WITH expired AS (\
                SELECT ctid FROM target_telemetry_samples \
                WHERE collected_at < $1 ORDER BY collected_at LIMIT $2\
             ) DELETE FROM target_telemetry_samples AS samples \
               USING expired WHERE samples.ctid = expired.ctid",
        )
        .bind(cutoff)
        .bind(RETENTION_DELETE_BATCH_SIZE)
        .execute(&self.pool)
        .await?
        .rows_affected();
        let docker = sqlx::query(
            "WITH expired AS (\
                SELECT ctid FROM target_docker_telemetry_samples \
                WHERE collected_at < $1 ORDER BY collected_at LIMIT $2\
             ) DELETE FROM target_docker_telemetry_samples AS samples \
               USING expired WHERE samples.ctid = expired.ctid",
        )
        .bind(cutoff)
        .bind(RETENTION_DELETE_BATCH_SIZE)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(system + docker)
    }

    pub async fn list_recent(
        &self,
        target_id: TargetId,
    ) -> Result<Vec<SystemTelemetrySample>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM target_telemetry_samples WHERE target_id=$1 AND collected_at >= $2 ORDER BY collected_at")
            .bind(target_id.as_uuid()).bind(Utc::now() - chrono::Duration::seconds(lxcup_core::telemetry::HISTORY_WINDOW_SECONDS)).fetch_all(&self.pool).await?;
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

    pub async fn list_docker_recent(
        &self,
        target_id: TargetId,
    ) -> Result<Vec<DockerTelemetrySample>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM target_docker_telemetry_samples WHERE target_id=$1 AND collected_at >= $2 ORDER BY collected_at, container_id")
            .bind(target_id.as_uuid())
            .bind(Utc::now() - chrono::Duration::seconds(lxcup_core::telemetry::HISTORY_WINDOW_SECONDS))
            .fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "Docker telemetry sample",
                    }
                })
            })
            .collect()
    }
}
