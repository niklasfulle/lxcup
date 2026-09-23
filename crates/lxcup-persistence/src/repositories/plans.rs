use super::*;

impl UpdatePlanRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, plan: &UpdatePlan) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO update_plans (id, container_id, status, created_at, plan_hash) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(plan.id.as_uuid())
        .bind(plan.container_id.value() as i64)
        .bind(plan_status_to_db(plan.status))
        .bind(plan.created_at)
        .bind(plan.plan_hash.as_deref())
        .execute(&mut *transaction)
        .await?;

        for package in &plan.requested_packages {
            sqlx::query(
                "INSERT INTO update_plan_requested_packages (plan_id, package_name) VALUES ($1, $2)",
            )
            .bind(plan.id.as_uuid())
            .bind(package.as_str())
            .execute(&mut *transaction)
            .await?;
        }
        for change in &plan.resolved_changes {
            sqlx::query(
                "INSERT INTO update_plan_changes (plan_id, package_name, from_version, to_version, kind) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(plan.id.as_uuid())
            .bind(change.package.as_str())
            .bind(change.from_version.as_ref().map(PackageVersion::as_str))
            .bind(change.to_version.as_ref().map(PackageVersion::as_str))
            .bind(change_kind_to_db(change.kind))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn find_by_id(
        &self,
        id: UpdatePlanId,
    ) -> Result<Option<UpdatePlan>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT id, container_id, status, created_at, plan_hash FROM update_plans WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let packages = sqlx::query(
            "SELECT package_name FROM update_plan_requested_packages WHERE plan_id = $1 ORDER BY package_name",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;
        let changes = sqlx::query(
            "SELECT package_name, from_version, to_version, kind FROM update_plan_changes WHERE plan_id = $1 ORDER BY package_name, kind",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        let mut plan = plan_from_row(row)?;
        plan.requested_packages = packages
            .into_iter()
            .map(|row| package_name(row.get("package_name")))
            .collect::<Result<_, _>>()?;
        plan.resolved_changes = changes
            .into_iter()
            .map(resolved_change_from_row)
            .collect::<Result<_, _>>()?;
        Ok(Some(plan))
    }

    /// Liefert die Planhistorie eines Containers chronologisch aufsteigend.
    pub async fn list_by_container(
        &self,
        container_id: ContainerId,
    ) -> Result<Vec<UpdatePlan>, RepositoryError> {
        let ids = sqlx::query(
            "SELECT id FROM update_plans WHERE container_id = $1 ORDER BY created_at, id",
        )
        .bind(container_id.value() as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut plans = Vec::with_capacity(ids.len());
        for row in ids {
            let id = UpdatePlanId::from_uuid(row.get("id"));
            if let Some(plan) = self.find_by_id(id).await? {
                plans.push(plan);
            }
        }
        Ok(plans)
    }
}
