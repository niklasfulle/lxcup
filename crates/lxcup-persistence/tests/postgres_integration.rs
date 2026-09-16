use std::time::Duration;

use chrono::Utc;
use lxcup_core::{Execution, ExecutionStatus};
use lxcup_persistence::{
    AuditEvent, Database, DatabaseConfig, ExecutionEvent, Repositories, seeds::seed_development,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn postgres_round_trip_uses_only_the_explicit_test_database() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };

    let config = DatabaseConfig::from_values(
        database_url,
        3,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .expect("test database configuration is valid");
    let database = Database::connect(&config)
        .await
        .expect("test database must be reachable");
    database
        .migrate()
        .await
        .expect("test migrations must succeed");

    let seed = seed_development(database.pool())
        .await
        .expect("development seed must succeed");
    let repositories = Repositories::new(&database);

    let loaded_node = repositories
        .nodes
        .find_by_id(seed.node.id)
        .await
        .expect("node read must succeed")
        .expect("seeded node must exist");
    assert_eq!(loaded_node.name, seed.node.name);

    let checked_at = Utc::now();
    let mut checked_node = loaded_node.clone();
    checked_node.record_check_success(
        "8.4.1",
        vec!["version.info".to_owned(), "node.info".to_owned()],
        checked_at,
    );
    repositories
        .nodes
        .update(&checked_node)
        .await
        .expect("successful node check must be persisted");
    let loaded_checked_node = repositories
        .nodes
        .find_by_id(seed.node.id)
        .await
        .expect("checked node read must succeed")
        .expect("checked node must exist");
    assert_eq!(
        loaded_checked_node.status,
        lxcup_core::NodeStatus::Connected
    );
    assert_eq!(
        loaded_checked_node.proxmox_version.as_deref(),
        Some("8.4.1")
    );
    assert_eq!(loaded_checked_node.capabilities.len(), 2);

    checked_node.record_check_failure("permission denied", Utc::now());
    repositories
        .nodes
        .update(&checked_node)
        .await
        .expect("failed node check must be persisted");
    let loaded_failed_node = repositories
        .nodes
        .find_by_id(seed.node.id)
        .await
        .expect("failed node read must succeed")
        .expect("failed node must exist");
    assert_eq!(
        loaded_failed_node.status,
        lxcup_core::NodeStatus::Disconnected
    );
    assert_eq!(
        loaded_failed_node.last_check_error.as_deref(),
        Some("permission denied")
    );

    let scan = lxcup_test_support::succeeded_scan();
    repositories
        .scans
        .save(&scan)
        .await
        .expect("scan write must succeed");
    let loaded_scan = repositories
        .scans
        .find_by_id(scan.id)
        .await
        .expect("scan read must succeed")
        .expect("saved scan must exist");
    assert_eq!(loaded_scan.updates.len(), 2);

    let mut plan = lxcup_test_support::ready_plan();
    plan.refresh_hash();
    repositories
        .plans
        .save(&plan)
        .await
        .expect("plan write must succeed");
    let loaded_plan = repositories
        .plans
        .find_by_id(plan.id)
        .await
        .expect("plan read must succeed")
        .expect("saved plan must exist");
    assert!(plan.has_same_content_as(&loaded_plan));

    let mut execution = Execution::new(plan.id);
    let now = Utc::now();
    execution
        .transition_to(ExecutionStatus::Running, now)
        .unwrap();
    execution
        .transition_to(ExecutionStatus::Succeeded, now)
        .unwrap();
    repositories
        .executions
        .save(&execution)
        .await
        .expect("execution write must succeed");
    let event = ExecutionEvent {
        id: Uuid::new_v4(),
        execution_id: execution.id,
        sequence: 1,
        event_type: "execution.completed".to_owned(),
        message: Some("test execution completed".to_owned()),
        fields: json!({"result": "success"}),
        created_at: now,
    };
    repositories
        .executions
        .append_event(&event)
        .await
        .expect("execution event write must succeed");
    let audit = AuditEvent {
        id: Uuid::new_v4(),
        node_id: Some(seed.node.id),
        container_id: Some(seed.container.id),
        plan_id: Some(plan.id),
        execution_id: Some(execution.id),
        event_type: "test.completed".to_owned(),
        details: json!({"source": "integration-test"}),
        created_at: now,
    };
    repositories
        .audit_events
        .append(&audit)
        .await
        .expect("audit event write must succeed");

    cleanup(
        &database,
        audit.id,
        event.id,
        execution.id,
        plan.id,
        scan.id,
    )
    .await
    .expect("test cleanup must succeed");
}

async fn cleanup(
    database: &Database,
    audit_id: Uuid,
    event_id: Uuid,
    execution_id: lxcup_core::ExecutionId,
    plan_id: lxcup_core::UpdatePlanId,
    scan_id: lxcup_core::ScanId,
) -> Result<(), sqlx::Error> {
    let pool = database.pool();
    sqlx::query("DELETE FROM audit_events WHERE id = $1")
        .bind(audit_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM execution_events WHERE id = $1")
        .bind(event_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM executions WHERE id = $1")
        .bind(execution_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plan_changes WHERE plan_id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plan_requested_packages WHERE plan_id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plans WHERE id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM scan_updates WHERE scan_id = $1")
        .bind(scan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM scans WHERE id = $1")
        .bind(scan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM containers WHERE id = $1")
        .bind(101_i64)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(Uuid::from_u128(0x10000000000000000000000000000001))
        .execute(pool)
        .await?;
    Ok(())
}
