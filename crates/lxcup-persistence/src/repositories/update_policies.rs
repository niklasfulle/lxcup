use super::{Database, RepositoryError, UpdatePolicy, UpdatePolicyRepository};
use sqlx::Row;

impl UpdatePolicyRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, policy: &UpdatePolicy) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(policy).map_err(RepositoryError::Serialization)?;
        sqlx::query("INSERT INTO update_policies (id, payload) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET payload = EXCLUDED.payload, updated_at = NOW()")
            .bind(&policy.id)
            .bind(payload)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<UpdatePolicy>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM update_policies ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "update policy",
                    }
                })
            })
            .collect()
    }
}
