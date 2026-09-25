use super::*;
use lxcup_core::{ResourceLifecycle, ResourceTarget};

#[tokio::test]
async fn logout_has_a_stable_stateless_contract() {
    let response = router(ApiState::new())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn schedules_validate_registered_targets_and_intervals() {
    let state = ApiState::new();
    let target = Target::new(
        "scheduled-target",
        TargetKind::LinuxServer,
        "192.0.2.80",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/schedules")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "id": "nightly-inventory",
                "operation": "collect_package_inventory",
                "timezone": "Europe/Berlin",
                "target_ids": [target_id],
                "every_minutes": 60,
                "enabled": true
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let list = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/schedules")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let duplicate = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/schedules")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "id": "nightly-inventory", "operation": "health_check", "timezone": "UTC",
                "target_ids": [target_id], "every_minutes": 0
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state).oneshot(duplicate).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn docker_discovery_schedule_can_be_created_and_paused_or_resumed() {
    let state = ApiState::new();
    let mut target = Target::new(
        "docker-host",
        TargetKind::Lxc,
        "192.0.2.90",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/schedules")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "id": "docker-nightly",
                "operation": "docker_discovery",
                "timezone": "Europe/Berlin",
                "target_ids": [target_id],
                "every_minutes": 30,
                "enabled": true
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(create)
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );

    let pause = Request::builder()
        .method(Method::PATCH)
        .uri("/api/v1/schedules/docker-nightly")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"enabled":false}"#))
        .unwrap();
    assert_eq!(
        router(state.clone()).oneshot(pause).await.unwrap().status(),
        StatusCode::OK
    );
    assert!(!state.store.read().await.schedules[0].enabled);

    let resume = Request::builder()
        .method(Method::PATCH)
        .uri("/api/v1/schedules/docker-nightly")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(resume)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(state.store.read().await.schedules[0].enabled);
}

#[tokio::test]
async fn docker_discovery_schedule_rejects_non_lxc_targets() {
    let state = ApiState::new();
    let target = Target::new(
        "linux-host",
        TargetKind::LinuxServer,
        "192.0.2.91",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/schedules")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "id": "docker-invalid",
                "operation": "docker_discovery",
                "timezone": "UTC",
                "target_ids": [target_id],
                "every_minutes": 30
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state).oneshot(request).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn scheduler_dispatches_due_inventory_once_and_advances_slot() {
    let state = ApiState::new();
    let mut target = Target::new(
        "scheduled-target",
        TargetKind::LinuxServer,
        "192.0.2.81",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    state.store.write().await.schedules.push(JobSchedule {
        id: "hourly-inventory".to_owned(),
        operation: "collect_package_inventory".to_owned(),
        timezone: "Europe/Berlin".to_owned(),
        target_ids: vec![target_id],
        frequency: ScheduleFrequency::EveryMinutes(60),
        enabled: true,
        threshold: None,
        policy_id: None,
        last_run_at: None,
        next_run_at: chrono::Utc::now() - chrono::Duration::minutes(1),
        last_error: None,
    });

    assert_eq!(state.dispatch_due_schedules().await, 1);
    assert_eq!(state.ansible.read().await.jobs().len(), 1);
    assert_eq!(state.dispatch_due_schedules().await, 0);
    let schedule = state.store.read().await.schedules[0].clone();
    assert!(schedule.last_run_at.is_some());
    assert!(schedule.next_run_at > chrono::Utc::now());
}

#[tokio::test]
async fn onboarding_reconciliation_queues_health_then_inventory_idempotently() {
    let state = ApiState::new();
    let target = Target::new(
        "onboarding-target",
        TargetKind::LinuxServer,
        "192.0.2.82",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential_ref = target.credential_secret_ref;
    let agent_ref = target.agent_secret_ref;
    state.store.write().await.targets.push(target);

    let deployment = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Pending,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: "0.2.0".to_owned(),
            },
            secret_refs: vec![credential_ref, agent_ref],
            idempotency_key: format!("onboarding-deploy-{}", target_id.as_uuid()),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(deployment) = deployment else {
        panic!("deployment should be newly created");
    };
    complete_in_memory_job(&state, deployment.id).await;

    assert_eq!(
        state.reconcile_onboarding_jobs().await,
        0,
        "follow-up work must wait for the target's first authenticated heartbeat"
    );
    state.store.write().await.targets[0].mark_managed();
    assert_eq!(state.reconcile_onboarding_jobs().await, 1);
    assert_eq!(state.reconcile_onboarding_jobs().await, 0);
    let jobs = state.ansible.read().await.jobs();
    let health = jobs
        .iter()
        .find(|job| job.operation == AnsibleOperation::HealthCheck)
        .expect("healthcheck must follow a successful deployment");
    assert_eq!(health.target, ResourceTarget::Target(target_id));
    complete_in_memory_job(&state, health.id).await;

    assert_eq!(state.reconcile_onboarding_jobs().await, 1);
    assert_eq!(state.reconcile_onboarding_jobs().await, 0);
    let jobs = state.ansible.read().await.jobs();
    let inventory = jobs
        .iter()
        .find(|job| job.operation == AnsibleOperation::CollectPackageInventory)
        .expect("package inventory must follow a successful healthcheck");
    assert_eq!(inventory.secret_refs, vec![credential_ref]);
    assert_eq!(jobs.len(), 3);
}

#[tokio::test]
async fn enrollment_poll_resumes_after_deployment_without_holding_the_store_lock() {
    let state = ApiState::new();
    let mut target = Target::new(
        "enrollment-resume-target",
        TargetKind::Lxc,
        "192.0.2.84",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential_ref = target.credential_secret_ref;
    let agent_ref = target.agent_secret_ref;
    let container = Container::new(
        ContainerId::new(184),
        NodeId::new(),
        "enrollment-resume-container",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.store.write().await.targets.push(target.clone());
    state.replace_containers(vec![container]).await;

    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/enrollments")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": 184,
                        "target_id": target_id,
                        "idempotency_key": "resume-enrollment"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let enrollment_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deployment = state
        .ansible
        .read()
        .await
        .jobs()
        .into_iter()
        .find(|job| job.idempotency_key == format!("enrollment-{enrollment_id}"))
        .expect("enrollment deployment should be queued");
    assert_eq!(deployment.secret_refs, vec![credential_ref, agent_ref]);
    complete_in_memory_job(&state, deployment.id).await;
    target.mark_managed();
    state.store.write().await.targets[0] = target;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        router(state.clone()).oneshot(
            Request::builder()
                .uri(format!("/api/v1/enrollments/{enrollment_id}"))
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("enrollment polling must not deadlock while queuing follow-up jobs")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let jobs = state.ansible.read().await.jobs();
    assert!(jobs.iter().any(|job| {
        job.operation == AnsibleOperation::HealthCheck
            && job.target == ResourceTarget::Target(target_id)
    }));
}

#[tokio::test]
async fn retry_requeues_the_same_failed_job_once_and_keeps_its_event_history() {
    let state = ApiState::new();
    let target = Target::new(
        "retry-target",
        TargetKind::LinuxServer,
        "192.0.2.83",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            secret_refs: Vec::new(),
            idempotency_key: "retry-healthcheck".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(job) = submission else {
        panic!("job should be newly created");
    };
    {
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(job.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(job.id, AnsibleJobStatus::Failed)
            .unwrap();
    }

    let retry = || {
        Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/ansible/jobs/{}/retry", job.id.as_uuid()))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "confirmed": true }).to_string(),
            ))
            .unwrap()
    };
    let response = router(state.clone()).oneshot(retry()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::Queued
    );
    assert_eq!(
        router(state.clone())
            .oneshot(retry())
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST,
        "an already requeued job cannot be queued a second time"
    );
    let events = state.ansible.read().await.events(job.id).unwrap();
    assert!(matches!(
        events.last().map(|event| &event.event),
        Some(lxcup_ansible::JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Queued
        })
    ));
}

async fn complete_in_memory_job(state: &ApiState, id: lxcup_core::AnsibleJobId) {
    let mut coordinator = state.ansible.write().await;
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::Succeeded,
    ] {
        coordinator.transition(id, status).unwrap();
    }
}

#[tokio::test]
async fn removed_nodes_endpoint_is_not_available() {
    let state = ApiState::new();
    state
        .replace_nodes(vec![Node::new("pve01", "https://pve01:8006").unwrap()])
        .await;
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
