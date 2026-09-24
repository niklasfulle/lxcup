use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lxcup_ansible::{
    AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, ExecutionMode,
    JobSubmission,
};
use lxcup_core::{JobSchedule, Node, NodeId, ScheduleFrequency};
use lxcup_secrets::CreateSecret;
use tower::ServiceExt;

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
        TargetKind::LinuxServer,
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

#[tokio::test]
async fn inventory_endpoints_list_containers_start_scans_and_render_metrics() {
    let state = ApiState::new();
    let node = Node::new("inventory-node", "https://pve.example.test").unwrap();
    let node_id = node.id;
    let container = Container::new(
        ContainerId::new(202),
        node_id,
        "inventory-container",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_nodes(vec![node]).await;
    state.replace_containers(vec![container]).await;

    for request in [Request::builder()
        .uri("/api/v1/containers")
        .body(Body::empty())
        .unwrap()]
    {
        assert_eq!(
            router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/202/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let scan_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/202/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let run_without_agent = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/scans/{scan_id}/run"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_without_agent.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let plan_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/202/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.0",
                            "to_version": "3.1",
                            "kind": "Upgrade",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(plan_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let plan_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/202/plans")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/plans/{plan_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(!scan_id.is_empty());
}

#[tokio::test]
async fn inventory_endpoints_fail_closed_for_invalid_ids_and_unready_nodes() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let node = Node::new("disconnected-node", "https://pve.example.test").unwrap();
    let mut disconnected = node.clone();
    disconnected.status = lxcup_core::NodeStatus::Disconnected;
    state.replace_nodes(vec![disconnected]).await;

    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    for request in [
        Request::builder()
            .uri("/api/v1/containers/not-a-container/scans")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/v1/containers/not-a-container/plans")
            .body(Body::empty())
            .unwrap(),
    ] {
        assert_eq!(
            router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    let missing_scan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_scan.status(), StatusCode::NOT_FOUND);

    let invalid_plan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": [],
                        "dry_run_changes": []
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_plan.status(), StatusCode::BAD_REQUEST);

    let blocked_plan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.1",
                            "to_version": null,
                            "kind": "Remove",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked_plan.status(), StatusCode::CREATED);
    let blocked_body = axum::body::to_bytes(blocked_plan.into_body(), usize::MAX)
        .await
        .unwrap();
    let blocked_id =
        serde_json::from_slice::<serde_json::Value>(&blocked_body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
    let blocked_confirm = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/plans/{blocked_id}/confirm"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked_confirm.status(), StatusCode::BAD_REQUEST);

    for uri in [
        "/api/v1/plans/00000000-0000-0000-0000-000000000001",
        "/api/v1/executions/00000000-0000-0000-0000-000000000001",
        "/api/v1/executions/00000000-0000-0000-0000-000000000001/result",
    ] {
        assert_eq!(
            router(state.clone())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap(),)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/scans/00000000-0000-0000-0000-000000000001/run")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let abort_missing = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/abort")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(abort_missing.status(), StatusCode::BAD_REQUEST);
    let run_missing = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/run")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_missing.status(), StatusCode::NOT_FOUND);

    let reconcile = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/reconcile")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"observed":"unknown"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reconcile.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn execution_endpoints_confirm_run_reconcile_and_abort() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(404),
        NodeId::new(),
        "execution-test",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let plan_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/404/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.0",
                            "to_version": "3.1",
                            "kind": "Upgrade",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_response.status(), StatusCode::CREATED);
    let plan_body = axum::body::to_bytes(plan_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let plan_id = serde_json::from_slice::<serde_json::Value>(&plan_body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let confirm = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/plans/{plan_id}/confirm"))
                .header("x-actor", "test-suite")
                .header("idempotency-key", "execution-test-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirm.status(), StatusCode::ACCEPTED);
    let confirm_body = axum::body::to_bytes(confirm.into_body(), usize::MAX)
        .await
        .unwrap();
    let execution_id = serde_json::from_slice::<serde_json::Value>(&confirm_body).unwrap()["data"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        let info = lxcup_agent::AgentInfo {
            agent_id: "execution-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Windows,
            hostname: "execution-host".to_owned(),
            version: "0.2.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        };
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(info, "agent-token")),
        )
        .await
        .unwrap();
    });
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new(format!("http://{address}"), "agent-token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(ContainerId::new(404), RegisteredAgent { client });

    let run = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/run"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run.status(), StatusCode::OK);
    let run_body = axum::body::to_bytes(run.into_body(), usize::MAX)
        .await
        .unwrap();
    let run_json = serde_json::from_slice::<serde_json::Value>(&run_body).unwrap();
    assert_eq!(run_json["data"]["status"], "failed");

    let result = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/executions/{execution_id}/result"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);

    let reconciled = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/reconcile"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"observed":"succeeded"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reconciled.status(), StatusCode::OK);

    let abort = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/abort"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(abort.status(), StatusCode::BAD_REQUEST);
    agent_task.abort();
}

#[tokio::test]
async fn docker_inventory_endpoints_manage_in_memory_workloads() {
    let state = ApiState::new();
    state.store.write().await.docker_workloads.insert(
        (ContainerId::new(303), "docker-303".to_owned()),
        DockerWorkloadDto {
            host_container_id: ContainerId::new(303),
            id: "docker-303".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: None,
            labels: vec!["app=web".to_owned()],
            presence: "present".to_owned(),
            change_state: "new".to_owned(),
            management_state: "discovered".to_owned(),
            discovered_at: chrono::Utc::now(),
        },
    );
    let listed = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/303/docker/containers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    let adopted = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/303/docker/containers/docker-303/adopt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(adopted.status(), StatusCode::OK);

    let rejected = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/v1/containers/303/docker/containers/docker-303")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let removed = router(state)
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/v1/containers/303/docker/containers/docker-303")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);

    for request in [
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/303/docker/discover")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/303/docker/containers/missing/adopt")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/containers/303/docker/containers/missing")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap(),
    ] {
        assert_eq!(
            router(ApiState::new().with_auth_config(AuthConfig::disabled()))
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn docker_unavailable_is_audited_without_marking_existing_workloads_missing() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let host_id = ContainerId::new(606);
    let agent_token = "docker-unavailable-token";
    let info = lxcup_agent::AgentInfo {
        agent_id: "windows-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Windows,
        hostname: "docker-host".to_owned(),
        version: "0.2.0".to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(info, agent_token)),
        )
        .await
        .unwrap();
    });
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new(format!("http://{address}"), agent_token).unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(host_id, RegisteredAgent { client });
    state.store.write().await.docker_workloads.insert(
        (host_id, "previous-container".to_owned()),
        DockerWorkloadDto {
            host_container_id: host_id,
            id: "previous-container".to_owned(),
            name: "previous".to_owned(),
            image: "nginx:stable".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec![],
            started_at: None,
            labels: vec![],
            presence: "present".to_owned(),
            change_state: "unchanged".to_owned(),
            management_state: "managed".to_owned(),
            discovered_at: chrono::Utc::now(),
        },
    );

    let failed = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/606/docker/discover")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
    let latest = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/606/docker/discovery")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(latest.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["status"], "failed");
    assert_eq!(json["data"]["error_code"], "docker_unavailable");

    let inventory = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/606/docker/containers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inventory.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"][0]["presence"], "present");
    agent_task.abort();
}

#[tokio::test]
async fn enrollment_endpoint_is_idempotent_and_queryable() {
    let state = ApiState::new();
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let create_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": 101,
                    "idempotency_key": "request-1"
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let enrollment_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["state"], "requested");

    let second = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["data"]["id"], enrollment_id);

    let get = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/enrollments/{enrollment_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
}

#[tokio::test]
async fn enrollment_rejects_key_reuse_and_unknown_containers() {
    let state = ApiState::new();
    let containers = [101, 102]
        .into_iter()
        .map(|id| {
            Container::new(
                ContainerId::new(id),
                NodeId::new(),
                format!("test-lxc-{id}"),
                lxcup_core::OperatingSystem::Debian,
                lxcup_core::ContainerStatus::Running,
            )
            .unwrap()
        })
        .collect();
    state.replace_containers(containers).await;

    let request = |container_id, key| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": container_id,
                    "idempotency_key": key
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(request(101, "request-1"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);

    let reused = router(state.clone())
        .oneshot(request(102, "request-1"))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);

    let unknown = router(state)
        .oneshot(request(999, "request-2"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ansible_job_endpoint_accepts_healthcheck_and_is_idempotent() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;
    let request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/ansible/jobs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "operation": "health_check",
                    "container_id": 101,
                    "mode": "check",
                    "parameters": {"operation": "health_check"},
                    "idempotency_key": "health-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let job_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["status"], "queued");

    let duplicate = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_body = axum::body::to_bytes(duplicate.into_body(), usize::MAX)
        .await
        .unwrap();
    let duplicate_json: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
    assert_eq!(duplicate_json["data"]["id"], job_id);

    let events = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);

    let status = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
}

#[tokio::test]
async fn configured_auth_protects_api_but_not_health() {
    let state = ApiState::new().with_auth_config(
        AuthConfig::disabled()
            .with_tokens(Some("viewer".to_owned()), Some("operator".to_owned()), None)
            .required(true),
    );
    let api_response = router(state.clone())
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api_response.status(), StatusCode::UNAUTHORIZED);

    let health_response = router(state)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn worker_availability_reports_unavailable_without_a_recent_heartbeat() {
    let response = router(ApiState::new().with_auth_config(AuthConfig::disabled()))
        .oneshot(
            Request::builder()
                .uri("/api/v1/ansible/worker-availability")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["available"], false);
}

#[tokio::test]
async fn agent_registration_exposes_health_and_metrics() {
    let agent_token = "test-agent-token";
    let agent_info = lxcup_agent::AgentInfo {
        agent_id: "test-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Linux,
        hostname: "test-lxc".to_owned(),
        version: "0.2.0".to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(agent_info, agent_token)),
        )
        .await
        .unwrap();
    });

    let secret_store = InMemorySecretStore::default();
    let agent_secret = secret_store
        .create(CreateSecret {
            name: "test-agent".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Container(ContainerId::new(101)),
            value: lxcup_core::SecretValue::new(agent_token).unwrap(),
        })
        .unwrap();
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let registration = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/agent")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "endpoint": format!("http://{address}"),
                "secret_ref": agent_secret.metadata.id,
            })
            .to_string(),
        ))
        .unwrap();
    let registration_response = router(state.clone()).oneshot(registration).await.unwrap();
    assert_eq!(registration_response.status(), StatusCode::CREATED);

    let health_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/101/agent/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), StatusCode::OK);
    let health_body = axum::body::to_bytes(health_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let health_json: serde_json::Value = serde_json::from_slice(&health_body).unwrap();
    assert_eq!(health_json["data"]["healthy"], true);
    assert_eq!(health_json["data"]["info"]["agent_id"], "test-agent");

    let metrics_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/101/agent/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics_response.status(), StatusCode::OK);
    let metrics_body = axum::body::to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_json: serde_json::Value = serde_json::from_slice(&metrics_body).unwrap();
    assert_eq!(metrics_json["data"]["commands_total"], 0);
    assert_eq!(metrics_json["data"]["commands_failed"], 0);

    let revoke = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/agent/revoke")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state).oneshot(revoke).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    agent_task.abort();
}

#[tokio::test]
async fn agent_heartbeat_updates_target_state_without_activity_event() {
    let agent_token = "heartbeat-token";
    let secret_store = InMemorySecretStore::default();
    let secret = secret_store
        .create(CreateSecret {
            name: "heartbeat-agent".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new(agent_token).unwrap(),
        })
        .unwrap();
    let target = Target::new(
        "heartbeat-target",
        TargetKind::LinuxServer,
        "192.0.2.77",
        TargetTransport::Ssh,
        SecretId::new(),
        secret.metadata.id,
    )
    .unwrap();
    let target_id = target.id;
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    state.store.write().await.targets.push(target);
    let mut events = state.events.subscribe();

    let now = chrono::Utc::now();
    let heartbeat = lxcup_agent::AgentHeartbeat {
        target_id: target_id.as_uuid(),
        info: lxcup_agent::AgentInfo {
            agent_id: "heartbeat-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Linux,
            hostname: "heartbeat-host".to_owned(),
            version: "0.2.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        },
        metrics: lxcup_agent::AgentMetrics {
            collected_at: now,
            commands_total: 1,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: lxcup_agent::SystemTelemetryWindow {
            samples: vec![
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now,
                    cpu_basis_points: Some(4_200),
                    memory_basis_points: Some(5_000),
                    storage_basis_points: Some(6_000),
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now - chrono::Duration::seconds(1),
                    cpu_basis_points: Some(3_500),
                    memory_basis_points: Some(4_500),
                    storage_basis_points: Some(5_500),
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now,
                    cpu_basis_points: Some(10_001),
                    memory_basis_points: None,
                    storage_basis_points: None,
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now - chrono::Duration::seconds(31),
                    cpu_basis_points: None,
                    memory_basis_points: None,
                    storage_basis_points: None,
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
            ],
            partial: false,
        },
    };
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/agents/heartbeat")
                .header("authorization", format!("Bearer {agent_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&heartbeat).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let store = state.store.read().await;
    assert_eq!(store.targets[0].state, TargetState::Managed);
    assert!(store.agent_reports.contains_key(&target_id));
    let received_telemetry = &store.agent_reports[&target_id].telemetry;
    assert!(received_telemetry.partial);
    assert_eq!(received_telemetry.samples.len(), 2);
    assert!(
        received_telemetry.samples[0].collected_at < received_telemetry.samples[1].collected_at
    );
    drop(store);

    let telemetry_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/targets/{}/telemetry", target_id.as_uuid()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(telemetry_response.status(), StatusCode::OK);
    let telemetry_body = axum::body::to_bytes(telemetry_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let telemetry_json: serde_json::Value = serde_json::from_slice(&telemetry_body).unwrap();
    assert_eq!(
        telemetry_json["data"]["samples"].as_array().unwrap().len(),
        2
    );

    let targets = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(targets.status(), StatusCode::OK);
    let targets_body = axum::body::to_bytes(targets.into_body(), usize::MAX)
        .await
        .unwrap();
    let targets_json: serde_json::Value = serde_json::from_slice(&targets_body).unwrap();
    assert_eq!(targets_json["data"][0]["agent_version"], "0.2.0");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn package_inventory_endpoint_reports_an_uncollected_target_without_packages() {
    let target = Target::new(
        "inventory-target",
        TargetKind::LinuxServer,
        "192.0.2.99",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let state = ApiState::new();
    state.store.write().await.targets.push(target);

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/targets/{}/package-inventory",
                    target_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["status"], "not_collected");
    assert_eq!(json["data"]["packages"], serde_json::json!([]));
    assert!(json["data"]["collected_at"].is_null());
}

#[tokio::test]
async fn agent_endpoints_surface_unavailable_clients_and_confirmation_errors() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container_id = ContainerId::new(505);
    let container = Container::new(
        container_id,
        NodeId::new(),
        "unavailable-agent",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new("http://127.0.0.1:1", "token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });

    for uri in [
        "/api/v1/containers/505/agent/health",
        "/api/v1/containers/505/agent/metrics",
    ] {
        let response = router(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    let not_confirmed = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_confirmed.status(), StatusCode::BAD_REQUEST);

    let revoked = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let second_revoke = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_revoke.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn agent_registration_requires_a_secret_reference_and_known_container() {
    let secret_store = InMemorySecretStore::default();
    let agent_secret = secret_store
        .create(CreateSecret {
            name: "agent-config-test".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new("token").unwrap(),
        })
        .unwrap();
    let ca_secret = secret_store
        .create(CreateSecret {
            name: "agent-ca-test".to_owned(),
            kind: lxcup_core::SecretKind::Generic,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new("not-a-pem").unwrap(),
        })
        .unwrap();
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    let container = Container::new(
        ContainerId::new(506),
        NodeId::new(),
        "secret-required-agent",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let missing_secret = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/506/agent")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"endpoint":"http://127.0.0.1:1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_secret.status(), StatusCode::BAD_REQUEST);

    let invalid_ca = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/506/agent")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "endpoint": "http://127.0.0.1:1",
                        "secret_ref": agent_secret.metadata.id,
                        "ca_secret_ref": ca_secret.metadata.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_ca.status(), StatusCode::BAD_GATEWAY);

    let missing_container = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/507/agent")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "endpoint": "http://127.0.0.1:1",
                        "secret_ref": SecretId::new()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_container.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[allow(unreachable_code)]
async fn removed_environment_api_is_not_available() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/environments")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "name": "pve-test",
                "endpoint": "https://pve.example.test/",
                "api_secret_ref": SecretId::new(),
                "ca_secret_ref": null
            })
            .to_string(),
        ))
        .unwrap();
    let response = router(state.clone()).oneshot(create).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    return;
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(json["data"]["endpoint"], "https://pve.example.test");
    assert!(!json.to_string().contains("token_secret"));

    let update = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/v1/environments/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"name": "pve-renamed"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(update)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let check = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/environments/{id}/check"))
        .body(Body::empty())
        .unwrap();
    let check_response = router(state.clone()).oneshot(check).await.unwrap();
    assert_eq!(check_response.status(), StatusCode::OK);
    let check_body = axum::body::to_bytes(check_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let check_json: serde_json::Value = serde_json::from_slice(&check_body).unwrap();
    assert_eq!(check_json["data"]["status"], "failed");

    let disable = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/environments/{id}/disable"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    let disable_response = router(state.clone()).oneshot(disable).await.unwrap();
    assert_eq!(disable_response.status(), StatusCode::OK);
    let delete = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/v1/environments/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state).oneshot(delete).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
#[allow(unreachable_code)]
async fn removed_container_action_api_is_not_available() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let stop_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/101/actions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "action": "shutdown",
                    "idempotency_key": "shutdown-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap()
    };
    let first = router(state.clone()).oneshot(stop_request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::NOT_FOUND);
    return;
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let task_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["status"], "queued");

    let duplicate = router(state.clone()).oneshot(stop_request()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);

    let start = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/actions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "action": "start",
                "idempotency_key": "start-1",
                "confirmed": false
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone()).oneshot(start).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    let status = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/container-actions/{task_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
}

#[tokio::test]
async fn secret_api_redacts_values_and_records_lifecycle_audit() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/secrets")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "name": "test-connection-token",
                "kind": "generic",
                "scope": {"type": "global"},
                "value": "never-return-this-value"
            })
            .to_string(),
        ))
        .unwrap();
    let response = router(state.clone()).oneshot(create).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = json["data"]["metadata"]["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!json.to_string().contains("never-return-this-value"));

    let rotate_without_confirmation = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/secrets/{id}/rotate"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"value":"rotated-value","confirmed":false}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(rotate_without_confirmation)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let revoke = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/secrets/{id}/revoke"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(revoke)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let audit = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/secrets/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit_body = axum::body::to_bytes(audit.into_body(), usize::MAX)
        .await
        .unwrap();
    let audit_json: serde_json::Value = serde_json::from_slice(&audit_body).unwrap();
    assert_eq!(audit_json["data"][0]["action"], "created");
    assert!(!audit_json.to_string().contains("never-return-this-value"));
}

#[test]
fn invalid_ids_are_rejected() {
    assert!(parse_container_id("0").is_err());
    assert!(parse_uuid("not-an-id", "plan id").is_err());
}
