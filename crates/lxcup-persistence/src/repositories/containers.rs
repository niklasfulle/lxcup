use super::{
    Container, ContainerId, ContainerRepository, Database, NodeId, RepositoryError,
    container_from_row, container_management_to_db, container_status_to_db, os_to_db,
};

impl ContainerRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, container: &Container) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO containers (id, node_id, name, operating_system, status, management_state, discovered_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(container.id.value() as i64)
        .bind(container.node_id.as_uuid())
        .bind(&container.name)
        .bind(os_to_db(&container.operating_system))
        .bind(container_status_to_db(container.status))
        .bind(container_management_to_db(container.management_state))
        .bind(container.discovered_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Inserts a newly discovered container or refreshes only its inventory data.
    /// The management state is intentionally preserved on refresh.
    pub async fn upsert_discovered(&self, container: &Container) -> Result<bool, RepositoryError> {
        let inserted = sqlx::query(
            "INSERT INTO containers (id, node_id, name, operating_system, status, management_state, discovered_at) VALUES ($1, $2, $3, $4, $5, 'discovered', $6) ON CONFLICT (id) DO NOTHING",
        )
        .bind(container.id.value() as i64)
        .bind(container.node_id.as_uuid())
        .bind(&container.name)
        .bind(os_to_db(&container.operating_system))
        .bind(container_status_to_db(container.status))
        .bind(container.discovered_at)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;

        if !inserted {
            sqlx::query(
                "UPDATE containers SET node_id = $1, name = $2, operating_system = $3, status = $4, discovered_at = $5 WHERE id = $6",
            )
            .bind(container.node_id.as_uuid())
            .bind(&container.name)
            .bind(os_to_db(&container.operating_system))
            .bind(container_status_to_db(container.status))
            .bind(container.discovered_at)
            .bind(container.id.value() as i64)
            .execute(&self.pool)
            .await?;
        }

        Ok(inserted)
    }

    pub async fn update_management_state(
        &self,
        container: &Container,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE containers SET management_state = $1 WHERE id = $2")
            .bind(container_management_to_db(container.management_state))
            .bind(container.id.value() as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_status_unknown(&self, id: ContainerId) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE containers SET status = 'unknown' WHERE id = $1")
            .bind(id.value() as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: ContainerId) -> Result<Option<Container>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, node_id, name, operating_system, status, management_state, discovered_at FROM containers WHERE id = $1",
        )
        .bind(id.value() as i64)
        .fetch_optional(&self.pool)
        .await?;

        row.map(container_from_row).transpose()
    }

    pub async fn list_by_node(&self, node_id: NodeId) -> Result<Vec<Container>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, node_id, name, operating_system, status, management_state, discovered_at FROM containers WHERE node_id = $1 ORDER BY id",
        )
        .bind(node_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(container_from_row).collect()
    }
}
