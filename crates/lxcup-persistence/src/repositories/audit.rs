use super::{
    AuditEvent, AuditEventRepository, Database, ExecutionId, NodeId, RepositoryError, UpdatePlanId,
};

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
}
