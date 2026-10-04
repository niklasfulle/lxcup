use super::*;
use lxcup_core::{ResourceLifecycle, ResourceTarget};
use std::time::Duration;

async fn unresolved_apply(state: &ApiState) -> lxcup_core::AnsibleJobId {
    let target = Target::new(
        "uncertain-apply-target",
        TargetKind::LinuxServer,
        "192.0.2.191",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential = target
        .credential_secret_ref
        .expect("SSH target has deployment credentials");
    let agent = target.agent_secret_ref;
    state.store.write().await.targets.push(target);
    let created = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::RepairAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::RepairAgent,
            secret_refs: vec![credential, agent],
            idempotency_key: "test-uncertain-apply".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(source) = created else {
        panic!("source job should be created")
    };
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::ReconcileRequired,
    ] {
        state
            .ansible
            .write()
            .await
            .transition(source.id, status)
            .unwrap();
    }
    source.id
}

#[tokio::test]
async fn reconcile_endpoint_creates_one_linked_job_only_for_unclear_apply() {
    let state = ApiState::new();
    let source_id = unresolved_apply(&state).await;
    let uri = format!("/api/v1/ansible/jobs/{}/reconcile", source_id.as_uuid());

    let created = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let reconciliation_id = payload["data"]["id"].as_str().unwrap();
    assert_eq!(payload["data"]["mode"], "reconcile");
    assert_eq!(
        payload["data"]["reconciles_job_id"],
        source_id.as_uuid().to_string()
    );

    let repeated = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::OK);
    let repeated_payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(repeated.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated_payload["data"]["id"], reconciliation_id);
    assert_eq!(state.ansible.read().await.jobs().len(), 2);

    state
        .ansible
        .write()
        .await
        .transition(source_id, AnsibleJobStatus::Succeeded)
        .unwrap();
    let resolved = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/ansible/jobs/{}/reconcile",
                    source_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reconcile_endpoint_rejects_new_or_resolved_jobs() {
    let state = ApiState::new();
    unresolved_apply(&state).await;
    let target = Target::new(
        "new-apply-target",
        TargetKind::LinuxServer,
        "192.0.2.192",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential = target
        .credential_secret_ref
        .expect("SSH target has deployment credentials");
    let agent = target.agent_secret_ref;
    state.store.write().await.targets.push(target);
    let rejected_source = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::RepairAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::RepairAgent,
            secret_refs: vec![credential, agent],
            idempotency_key: "new-apply".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(rejected_source) = rejected_source else {
        panic!("second source job should be created")
    };
    let uri = format!(
        "/api/v1/ansible/jobs/{}/reconcile",
        rejected_source.id.as_uuid()
    );
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.ansible.read().await.jobs().len(), 2);
}

#[tokio::test]
async fn persistent_reconcile_ignores_its_source_apply_job_as_an_active_conflict() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("reconcile_route_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        2,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let state = ApiState::new()
        .with_repositories(repositories.clone())
        .with_auth_config(AuthConfig::disabled());
    let source_id = unresolved_apply(&state).await;
    let target = state.store.read().await.targets[0].clone();
    repositories.targets.save(&target).await.unwrap();
    let source = state.ansible.read().await.job(source_id).unwrap();
    repositories.ansible_jobs.save(&source).await.unwrap();

    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/ansible/jobs/{}/reconcile",
                    source_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let reconciliation_id = payload["data"]["id"].as_str().unwrap();
    assert_eq!(payload["data"]["mode"], "reconcile");
    assert_eq!(
        payload["data"]["reconciles_job_id"],
        source_id.as_uuid().to_string()
    );
    assert!(
        repositories
            .ansible_jobs
            .find_by_id(lxcup_core::AnsibleJobId::from_uuid(
                Uuid::parse_str(reconciliation_id).unwrap()
            ))
            .await
            .unwrap()
            .is_some()
    );

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn persistent_workflow_create_read_event_and_retry_paths_round_trip() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("workflow_routes_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        2,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let state = ApiState::new()
        .with_repositories(repositories.clone())
        .with_auth_config(AuthConfig::disabled());
    let mut target = Target::new(
        "persistent-workflow-target",
        TargetKind::LinuxServer,
        "192.0.2.193",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    repositories.targets.save(&target).await.unwrap();
    state.store.write().await.targets.push(target.clone());

    let (status, created) = create_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Operator),
        JsonBody(crate::workflows::CreateAnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target_id: Some(target.id),
            container_id: 0,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            idempotency_key: "persistent-workflow-routes".to_owned(),
            confirmed: true,
            policy_id: None,
            approved_plan_job_id: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);
    let job_id = created.0.data.id;
    let listed = list_ansible_jobs(State(state.clone())).await.unwrap();
    assert_eq!(listed.0.data.len(), 1);
    assert_eq!(listed.0.data[0].id, job_id);
    assert_eq!(
        get_ansible_job(State(state.clone()), Path(job_id.as_uuid().to_string()))
            .await
            .unwrap()
            .0
            .data
            .id,
        job_id
    );
    assert!(
        !get_ansible_job_events(State(state.clone()), Path(job_id.as_uuid().to_string()))
            .await
            .unwrap()
            .0
            .data
            .is_empty()
    );

    let mut failed = repositories
        .ansible_jobs
        .find_by_id(job_id)
        .await
        .unwrap()
        .unwrap();
    failed.transition_to(AnsibleJobStatus::Checking).unwrap();
    failed.transition_to(AnsibleJobStatus::Failed).unwrap();
    repositories.ansible_jobs.update(&failed).await.unwrap();
    let retry = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/ansible/jobs/{}/retry", job_id.as_uuid()))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    let retried: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(retry.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(retried["data"]["id"], job_id.as_uuid().to_string());
    assert_eq!(retried["data"]["status"], "queued");

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}
