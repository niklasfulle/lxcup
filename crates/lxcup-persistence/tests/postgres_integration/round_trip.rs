use super::*;

#[tokio::test]
async fn postgres_round_trip_uses_only_the_explicit_test_database() {
    let Some(test_database) = scoped_test_database("round_trip").await else {
        return;
    };
    let database = test_database.database();

    let seed = seed_development(database.pool())
        .await
        .expect("development seed must succeed");
    let repositories = Repositories::new(database);

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
    assert!(
        repositories
            .plans
            .find_by_id(lxcup_core::UpdatePlanId::new())
            .await
            .expect("missing plan lookup must succeed")
            .is_none()
    );
    assert!(
        repositories
            .plans
            .list_by_container(seed.container.id)
            .await
            .expect("container plan history lookup must succeed")
            .iter()
            .any(|saved| saved.id == plan.id)
    );

    let mut execution = Execution::new(plan.id);
    let now = postgres_now();
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
    let idempotency_key = format!("round-trip-{}", Uuid::new_v4());
    assert!(
        repositories
            .executions
            .claim_idempotency_key(&idempotency_key, execution.id)
            .await
            .expect("first execution request claim must succeed")
    );
    assert!(
        !repositories
            .executions
            .claim_idempotency_key(&idempotency_key, execution.id)
            .await
            .expect("duplicate execution request claim must be ignored")
    );
    assert_eq!(
        repositories
            .executions
            .find_by_idempotency_key(&idempotency_key)
            .await
            .expect("idempotency key read must succeed"),
        Some(execution.id)
    );
    repositories
        .executions
        .update(&execution)
        .await
        .expect("execution update must succeed");
    assert_eq!(
        repositories
            .executions
            .find_by_id(execution.id)
            .await
            .expect("execution read must succeed")
            .expect("saved execution must exist")
            .status,
        ExecutionStatus::Succeeded
    );
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
    let result = lxcup_persistence::ExecutionResultRecord {
        execution_id: execution.id,
        exit_code: 0,
        stdout: "first result".to_owned(),
        stderr: String::new(),
        created_at: now,
    };
    repositories
        .executions
        .save_result(&result)
        .await
        .expect("execution result write must succeed");
    let replacement_result = lxcup_persistence::ExecutionResultRecord {
        stdout: "updated result".to_owned(),
        ..result
    };
    repositories
        .executions
        .save_result(&replacement_result)
        .await
        .expect("execution result replacement must succeed");
    assert_eq!(
        repositories
            .executions
            .find_result(execution.id)
            .await
            .expect("execution result read must succeed")
            .expect("saved execution result must exist")
            .stdout,
        "updated result"
    );
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
    let secret_event = AuditEvent {
        id: Uuid::new_v4(),
        node_id: None,
        container_id: None,
        plan_id: None,
        execution_id: None,
        event_type: "secret.created".to_owned(),
        details: json!({ "name": "test-secret", "value": "must-not-be-returned" }),
        created_at: now + chrono::Duration::seconds(1),
    };
    repositories
        .audit_events
        .append(&secret_event)
        .await
        .expect("secret audit event write must succeed");
    let secret_events = repositories
        .audit_events
        .list_secret_events()
        .await
        .expect("secret audit events must be queryable");
    assert!(secret_events.iter().any(|event| {
        event.id == secret_event.id
            && event.node_id.is_none()
            && event.container_id.is_none()
            && event.plan_id.is_none()
            && event.execution_id.is_none()
            && event.details["name"] == "test-secret"
    }));
    sqlx::query("DELETE FROM audit_events WHERE id = $1")
        .bind(secret_event.id)
        .execute(database.pool())
        .await
        .expect("secret audit test event cleanup must succeed");
    sqlx::query("DELETE FROM execution_requests WHERE idempotency_key = $1")
        .bind(&idempotency_key)
        .execute(database.pool())
        .await
        .expect("execution request test row cleanup must succeed");
    sqlx::query("DELETE FROM execution_results WHERE execution_id = $1")
        .bind(execution.id.as_uuid())
        .execute(database.pool())
        .await
        .expect("execution result test row cleanup must succeed");

    cleanup(database, audit.id, event.id, execution.id, plan.id, scan.id)
        .await
        .expect("test cleanup must succeed");
    test_database.finish().await;
}
