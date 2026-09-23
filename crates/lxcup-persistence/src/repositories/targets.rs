use super::{Database, RepositoryError, Target, TargetId, TargetRepository, target_from_row};

impl TargetRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, target: &Target) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(target).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO targets (id, name, kind, address, transport, state, payload, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(target.id.as_uuid())
        .bind(&target.name)
        .bind(format!("{:?}", target.kind).to_lowercase())
        .bind(&target.address)
        .bind(format!("{:?}", target.transport).to_lowercase())
        .bind(format!("{:?}", target.state).to_lowercase())
        .bind(payload)
        .bind(target.created_at)
        .bind(target.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, target: &Target) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(target).map_err(RepositoryError::Serialization)?;
        sqlx::query("UPDATE targets SET name = $1, kind = $2, address = $3, transport = $4, state = $5, payload = $6, updated_at = $7 WHERE id = $8")
            .bind(&target.name)
            .bind(format!("{:?}", target.kind).to_lowercase())
            .bind(&target.address)
            .bind(format!("{:?}", target.transport).to_lowercase())
            .bind(format!("{:?}", target.state).to_lowercase())
            .bind(payload)
            .bind(target.updated_at)
            .bind(target.id.as_uuid())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: TargetId) -> Result<Option<Target>, RepositoryError> {
        let row = sqlx::query("SELECT payload FROM targets WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;
        row.map(target_from_row).transpose()
    }

    pub async fn list(&self) -> Result<Vec<Target>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM targets ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(target_from_row).collect()
    }
}
