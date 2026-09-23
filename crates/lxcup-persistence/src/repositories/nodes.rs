use super::*;

impl NodeRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, node: &Node) -> Result<(), RepositoryError> {
        let capabilities =
            serde_json::to_value(&node.capabilities).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO nodes (id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(node.id.as_uuid())
        .bind(&node.name)
        .bind(&node.address)
        .bind(node_status_to_db(node.status))
        .bind(node.proxmox_version.as_deref())
        .bind(capabilities)
        .bind(node.last_checked_at)
        .bind(node.last_check_error.as_deref())
        .bind(node.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, node: &Node) -> Result<(), RepositoryError> {
        let capabilities =
            serde_json::to_value(&node.capabilities).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "UPDATE nodes SET name = $1, address = $2, status = $3, proxmox_version = $4, capabilities = $5, last_checked_at = $6, last_check_error = $7, created_at = $8 WHERE id = $9",
        )
        .bind(&node.name)
        .bind(&node.address)
        .bind(node_status_to_db(node.status))
        .bind(node.proxmox_version.as_deref())
        .bind(capabilities)
        .bind(node.last_checked_at)
        .bind(node.last_check_error.as_deref())
        .bind(node.created_at)
        .bind(node.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: NodeId) -> Result<Option<Node>, RepositoryError> {
        let row =
            sqlx::query("SELECT id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at FROM nodes WHERE id = $1")
                .bind(id.as_uuid())
                .fetch_optional(&self.pool)
                .await?;

        row.map(node_from_row).transpose()
    }

    pub async fn list(&self) -> Result<Vec<Node>, RepositoryError> {
        let rows =
            sqlx::query("SELECT id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at FROM nodes ORDER BY name")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter().map(node_from_row).collect()
    }
}
