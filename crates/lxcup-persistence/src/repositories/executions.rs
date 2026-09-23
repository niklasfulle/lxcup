use super::{
    Database, Execution, ExecutionEvent, ExecutionId, ExecutionRepository, ExecutionResultRecord,
    RepositoryError, Row, execution_from_row, execution_status_to_db,
};

impl ExecutionRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, execution: &Execution) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO executions (id, plan_id, status, started_at, finished_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(execution.id.as_uuid())
        .bind(execution.plan_id.as_uuid())
        .bind(execution_status_to_db(execution.status))
        .bind(execution.started_at)
        .bind(execution.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, execution: &Execution) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE executions SET plan_id = $1, status = $2, started_at = $3, finished_at = $4 WHERE id = $5",
        )
        .bind(execution.plan_id.as_uuid())
        .bind(execution_status_to_db(execution.status))
        .bind(execution.started_at)
        .bind(execution.finished_at)
        .bind(execution.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Claims an idempotency key exactly once for an execution.
    pub async fn claim_idempotency_key(
        &self,
        key: &str,
        execution_id: ExecutionId,
    ) -> Result<bool, RepositoryError> {
        let inserted = sqlx::query(
            "INSERT INTO execution_requests (idempotency_key, execution_id) VALUES ($1, $2) ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(key)
        .bind(execution_id.as_uuid())
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(inserted)
    }

    pub async fn find_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<ExecutionId>, RepositoryError> {
        let row =
            sqlx::query("SELECT execution_id FROM execution_requests WHERE idempotency_key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| ExecutionId::from_uuid(row.get("execution_id"))))
    }

    pub async fn find_by_id(&self, id: ExecutionId) -> Result<Option<Execution>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, plan_id, status, started_at, finished_at FROM executions WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;

        row.map(execution_from_row).transpose()
    }

    pub async fn append_event(&self, event: &ExecutionEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO execution_events (id, execution_id, sequence, event_type, message, fields, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(event.id)
        .bind(event.execution_id.as_uuid())
        .bind(event.sequence)
        .bind(&event.event_type)
        .bind(event.message.as_deref())
        .bind(&event.fields)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_result(&self, result: &ExecutionResultRecord) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO execution_results (execution_id, exit_code, stdout, stderr, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (execution_id) DO UPDATE SET exit_code = EXCLUDED.exit_code, stdout = EXCLUDED.stdout, stderr = EXCLUDED.stderr, created_at = EXCLUDED.created_at",
        )
        .bind(result.execution_id.as_uuid())
        .bind(result.exit_code)
        .bind(&result.stdout)
        .bind(&result.stderr)
        .bind(result.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_result(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Option<ExecutionResultRecord>, RepositoryError> {
        let row = sqlx::query("SELECT execution_id, exit_code, stdout, stderr, created_at FROM execution_results WHERE execution_id = $1")
            .bind(execution_id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(ExecutionResultRecord {
                execution_id: ExecutionId::from_uuid(row.try_get("execution_id")?),
                exit_code: row.try_get("exit_code")?,
                stdout: row.try_get("stdout")?,
                stderr: row.try_get("stderr")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .transpose()
    }
}
