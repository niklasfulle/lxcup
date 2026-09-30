use super::{Database, RepositoryError, Target, TargetId, TargetRepository, target_from_row};
use sqlx::Row;

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

    /// Removes a target and all target-scoped operational data atomically.
    /// A minimal audit tombstone is retained; secrets remain in their separate
    /// store because they may be shared by other resources.
    pub async fn delete_with_related_data(
        &self,
        id: TargetId,
        actor_role: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let target = sqlx::query("SELECT name FROM targets WHERE id = $1 FOR UPDATE")
            .bind(id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await?;
        let Some(target) = target else {
            return Ok(false);
        };
        let name: String = target.try_get("name")?;

        let target_jobs =
            sqlx::query("SELECT status FROM ansible_jobs WHERE target_id = $1 FOR UPDATE")
                .bind(id.as_uuid())
                .fetch_all(&mut *transaction)
                .await?;
        let has_active_jobs = target_jobs.iter().try_fold(false, |active, row| {
            let status: String = row.try_get("status")?;
            Ok::<_, sqlx::Error>(
                active
                    || matches!(
                        status.as_str(),
                        "checking" | "planned" | "applying" | "reconcile_required"
                    ),
            )
        })?;
        if has_active_jobs {
            return Err(RepositoryError::TargetBusy);
        }

        sqlx::query(
            "UPDATE schedules SET target_ids = array_remove(target_ids, $1), updated_at = NOW() WHERE $1 = ANY(target_ids)",
        )
        .bind(id.as_uuid())
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM schedules WHERE cardinality(target_ids) = 0")
            .execute(&mut *transaction)
            .await?;

        let target_as_json = serde_json::json!([id.as_uuid().to_string()]);
        let emptied_policy_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM update_policies WHERE payload->'allowed_targets' @> $1 AND jsonb_array_length(payload->'allowed_targets') = 1",
        )
        .bind(&target_as_json)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE update_policies SET payload = jsonb_set(payload, '{allowed_targets}', COALESCE((SELECT jsonb_agg(value) FROM jsonb_array_elements(payload->'allowed_targets') AS value WHERE value #>> '{}' <> $1::text), '[]'::jsonb), true), updated_at = NOW() WHERE payload->'allowed_targets' @> jsonb_build_array($1::text)",
        )
        .bind(id.as_uuid().to_string())
        .execute(&mut *transaction)
        .await?;
        if !emptied_policy_ids.is_empty() {
            let removed_policies = sqlx::query_scalar::<_, String>(
                "DELETE FROM update_policies WHERE id = ANY($1) RETURNING id",
            )
            .bind(&emptied_policy_ids)
            .fetch_all(&mut *transaction)
            .await?;
            sqlx::query("DELETE FROM schedules WHERE policy_id = ANY($1)")
                .bind(&removed_policies)
                .execute(&mut *transaction)
                .await?;
        }

        sqlx::query("DELETE FROM targets WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO audit_events (id, event_type, details, created_at) VALUES ($1, 'target.deleted', $2, NOW())",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(serde_json::json!({ "target_id": id.as_uuid(), "target_name": name, "role": actor_role }))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn list(&self) -> Result<Vec<Target>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM targets ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(target_from_row).collect()
    }
}
