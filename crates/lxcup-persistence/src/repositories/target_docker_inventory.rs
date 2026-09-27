use super::{Database, RepositoryError, TargetDockerInventoryRepository};
use chrono::{DateTime, Utc};
use lxcup_agent::DockerContainerInfo;
use lxcup_core::TargetId;
use sqlx::Row;

#[derive(Clone, Debug)]
pub struct PersistedTargetDockerInventory {
    pub collected_at: DateTime<Utc>,
    pub containers: Vec<DockerContainerInfo>,
}

impl TargetDockerInventoryRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(
        &self,
        target_id: TargetId,
        inventory: &PersistedTargetDockerInventory,
    ) -> Result<(), RepositoryError> {
        let containers =
            serde_json::to_value(&inventory.containers).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO target_docker_inventory (target_id, collected_at, containers) VALUES ($1, $2, $3) \
             ON CONFLICT (target_id) DO UPDATE SET collected_at = EXCLUDED.collected_at, containers = EXCLUDED.containers",
        )
        .bind(target_id.as_uuid())
        .bind(inventory.collected_at)
        .bind(containers)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        target_id: TargetId,
    ) -> Result<Option<PersistedTargetDockerInventory>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT collected_at, containers FROM target_docker_inventory WHERE target_id = $1",
        )
        .bind(target_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let containers: serde_json::Value = row.try_get("containers")?;
        Ok(Some(PersistedTargetDockerInventory {
            collected_at: row.try_get("collected_at")?,
            containers: serde_json::from_value(containers)
                .map_err(RepositoryError::Serialization)?,
        }))
    }
}
