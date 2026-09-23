use std::time::Duration;

use chrono::Utc;
use lxcup_ansible::{
    AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters,
    ExecutionMode, JobEvent, JobEventKind,
};
use lxcup_core::{
    ActorRole, AgentRegistration, Container, ContainerId, ContainerStatus, DockerWorkload,
    DockerWorkloadManagementState, EnvironmentId, Execution, ExecutionStatus, Node,
    OperatingSystem, ProxmoxEnvironment, ResourceLifecycle, ResourceTarget, SecretId, Target,
    TargetKind, TargetTransport,
};
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

#[tokio::test]
async fn postgres_repositories_cover_target_agent_environment_and_inventory_crud() {
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
    .unwrap();
    let database = Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = Repositories::new(&database);
    let now = Utc::now();
    let suffix = Uuid::new_v4().simple().to_string();
    let container_id = ContainerId::new((Uuid::new_v4().as_u128() as u64 % 900_000_000) + 10_000);
    let node = Node::new(
        format!("repository-crud-node-{suffix}"),
        "https://pve.example.test",
    )
    .unwrap();
    let node_id = node.id;
    repositories.nodes.save(&node).await.unwrap();
    let mut container = Container::new(
        container_id,
        node_id,
        format!("repository-crud-{suffix}"),
        OperatingSystem::Debian,
        ContainerStatus::Running,
    )
    .unwrap();
    assert!(
        repositories
            .containers
            .upsert_discovered(&container)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .containers
            .upsert_discovered(&container)
            .await
            .unwrap()
    );
    container
        .transition_management_to(lxcup_core::ContainerManagementState::Managed)
        .unwrap();
    repositories
        .containers
        .update_management_state(&container)
        .await
        .unwrap();
    repositories
        .containers
        .mark_status_unknown(container_id)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .containers
            .list_by_node(node_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let workload = DockerWorkload {
        host_container_id: container_id,
        id: format!("docker-crud-{suffix}"),
        name: "web".to_owned(),
        image: "nginx:latest".to_owned(),
        state: "running".to_owned(),
        status: "Up".to_owned(),
        management_state: DockerWorkloadManagementState::Discovered,
        discovered_at: now,
    };
    repositories
        .docker_workloads
        .upsert_discovered(&workload)
        .await
        .unwrap();
    assert!(
        repositories
            .docker_workloads
            .adopt(container_id, &workload.id)
            .await
            .unwrap()
    );
    assert_eq!(
        repositories
            .docker_workloads
            .list(container_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repositories
            .docker_workloads
            .remove(container_id, &workload.id)
            .await
            .unwrap()
    );

    let environment = ProxmoxEnvironment::new(
        EnvironmentId::new(),
        format!("crud-env-{suffix}"),
        "https://pve.example.test",
        SecretId::new(),
        Some(SecretId::new()),
        now,
    )
    .unwrap();
    repositories.environments.save(&environment).await.unwrap();
    assert!(
        repositories
            .environments
            .find_by_id(environment.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repositories
            .environments
            .list()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == environment.id)
    );
    repositories
        .environments
        .delete(environment.id)
        .await
        .unwrap();

    let registration = AgentRegistration::new(
        container_id,
        format!("agent-crud-{suffix}"),
        "http://127.0.0.1:8090",
        SecretId::new(),
        None,
        now,
    )
    .unwrap();
    repositories
        .agent_registrations
        .save(&registration)
        .await
        .unwrap();
    assert!(
        repositories
            .agent_registrations
            .find_by_container(container_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repositories
            .agent_registrations
            .list()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == registration.id)
    );

    let target = Target::new(
        format!("target-crud-{suffix}"),
        TargetKind::LinuxServer,
        "192.0.2.10",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    assert!(
        repositories
            .targets
            .find_by_id(target.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repositories
            .targets
            .list()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == target.id)
    );
    let mut managed = target.clone();
    managed.mark_managed();
    repositories.targets.update(&managed).await.unwrap();
    assert_eq!(
        repositories
            .targets
            .find_by_id(target.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        lxcup_core::TargetState::Managed
    );

    let pool = database.pool();
    sqlx::query("DELETE FROM targets WHERE id = $1")
        .bind(target.id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_registrations WHERE container_id = $1")
        .bind(container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM containers WHERE id = $1")
        .bind(container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(node_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn postgres_ansible_job_repository_claims_jobs_and_persists_events() {
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
    .unwrap();
    let database = Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = Repositories::new(&database);
    let target = Target::new(
        "ansible-job-repository-target",
        TargetKind::LinuxServer,
        "192.0.2.55",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let target_ref = ResourceTarget::Target(target.id);
    let job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "repository-job-test".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap();
    let job_id = job.id;
    repositories.ansible_jobs.save(&job).await.unwrap();
    assert_eq!(
        repositories
            .ansible_jobs
            .find_by_id(job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Queued
    );
    assert!(
        repositories
            .ansible_jobs
            .find_by_idempotency_key(target_ref, "repository-job-test")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert_eq!(
        repositories
            .ansible_jobs
            .list()
            .await
            .unwrap()
            .iter()
            .filter(|item| item.id == job_id)
            .count(),
        1
    );

    let claimed = repositories
        .ansible_jobs
        .claim_next_queued()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, job_id);
    assert_eq!(claimed.status, AnsibleJobStatus::Checking);
    assert!(
        repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert_eq!(
        repositories
            .ansible_jobs
            .claim_next_queued()
            .await
            .unwrap()
            .is_none(),
        true
    );
    let events = repositories.ansible_jobs.events(job_id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].event,
        JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Checking
        }
    ));

    let event = JobEvent {
        sequence: 2,
        job_id,
        event: JobEventKind::WorkerLog {
            source: "stdout".to_owned(),
            message: "health ok".to_owned(),
        },
        created_at: Utc::now(),
    };
    repositories
        .ansible_jobs
        .append_event(&event)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .ansible_jobs
            .events(job_id)
            .await
            .unwrap()
            .len(),
        2
    );

    let mut finished = claimed;
    finished.transition_to(AnsibleJobStatus::Planned).unwrap();
    finished.transition_to(AnsibleJobStatus::Failed).unwrap();
    repositories.ansible_jobs.update(&finished).await.unwrap();
    assert_eq!(
        repositories
            .ansible_jobs
            .find_by_id(job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Failed
    );
    assert!(
        !repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert!(
        repositories
            .ansible_jobs
            .find_by_idempotency_key(target_ref, "missing-key")
            .await
            .unwrap()
            .is_none()
    );

    let pool = database.pool();
    sqlx::query("DELETE FROM ansible_job_events WHERE job_id = $1")
        .bind(job_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ansible_jobs WHERE id = $1")
        .bind(job_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM targets WHERE id = $1")
        .bind(target.id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
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
