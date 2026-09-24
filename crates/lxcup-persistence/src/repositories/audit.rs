use super::{
    AuditEvent, AuditEventRepository, Database, ExecutionId, NodeId, RepositoryError, UpdatePlanId,
};
use sqlx::Row;

impl AuditEventRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn append(&self, event: &AuditEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO audit_events (id, node_id, container_id, plan_id, execution_id, event_type, details, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(event.id)
        .bind(event.node_id.map(NodeId::as_uuid))
        .bind(event.container_id.map(|id| id.value() as i64))
        .bind(event.plan_id.map(UpdatePlanId::as_uuid))
        .bind(event.execution_id.map(ExecutionId::as_uuid))
        .bind(&event.event_type)
        .bind(&event.details)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_secret_events(&self) -> Result<Vec<AuditEvent>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, event_type, details, created_at FROM audit_events WHERE event_type LIKE 'secret.%' ORDER BY created_at DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(AuditEvent {
                    id: row.try_get("id")?,
                    node_id: None,
                    container_id: None,
                    plan_id: None,
                    execution_id: None,
                    event_type: row.try_get("event_type")?,
                    details: row.try_get("details")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
}
