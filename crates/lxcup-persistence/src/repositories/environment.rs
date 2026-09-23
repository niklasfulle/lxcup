use super::*;

impl EnvironmentRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, environment: &ProxmoxEnvironment) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO proxmox_environments (id, name, endpoint, api_secret_ref, ca_secret_ref, status, last_checked_at, last_check_error, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(environment.id.as_uuid())
        .bind(&environment.name)
        .bind(&environment.endpoint)
        .bind(environment.api_secret_ref.as_uuid())
        .bind(environment.ca_secret_ref.map(SecretId::as_uuid))
        .bind(environment_status_to_db(environment.status))
        .bind(environment.last_checked_at)
        .bind(environment.last_check_error.as_deref())
        .bind(environment.created_at)
        .bind(environment.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, environment: &ProxmoxEnvironment) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE proxmox_environments SET name = $1, endpoint = $2, api_secret_ref = $3, ca_secret_ref = $4, status = $5, last_checked_at = $6, last_check_error = $7, updated_at = $8 WHERE id = $9",
        )
        .bind(&environment.name)
        .bind(&environment.endpoint)
        .bind(environment.api_secret_ref.as_uuid())
        .bind(environment.ca_secret_ref.map(SecretId::as_uuid))
        .bind(environment_status_to_db(environment.status))
        .bind(environment.last_checked_at)
        .bind(environment.last_check_error.as_deref())
        .bind(environment.updated_at)
        .bind(environment.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: lxcup_core::EnvironmentId) -> Result<(), RepositoryError> {
        sqlx::query("DELETE FROM proxmox_environments WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(
        &self,
        id: lxcup_core::EnvironmentId,
    ) -> Result<Option<ProxmoxEnvironment>, RepositoryError> {
        let row = sqlx::query("SELECT id, name, endpoint, api_secret_ref, ca_secret_ref, status, last_checked_at, last_check_error, created_at, updated_at FROM proxmox_environments WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;
        row.map(environment_from_row).transpose()
    }

    pub async fn list(&self) -> Result<Vec<ProxmoxEnvironment>, RepositoryError> {
        let rows = sqlx::query("SELECT id, name, endpoint, api_secret_ref, ca_secret_ref, status, last_checked_at, last_check_error, created_at, updated_at FROM proxmox_environments ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(environment_from_row).collect()
    }
}
