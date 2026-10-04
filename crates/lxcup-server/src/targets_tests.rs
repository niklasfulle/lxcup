use super::*;
use axum::Extension;
use uuid::Uuid;

fn test_target() -> Target {
    Target::new(
        "removal-test",
        TargetKind::Lxc,
        "192.0.2.77",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn target_removal_requires_admin_confirmation_and_clears_scoped_state() {
    let state = ApiState::new();
    let target = test_target();
    state.store.write().await.targets.push(target.clone());
    state.store.write().await.agent_reports.insert(
        target.id,
        lxcup_agent::AgentHeartbeat {
            target_id: target.id.as_uuid(),
            info: lxcup_agent::AgentInfo {
                agent_id: "agent-removal-test".to_owned(),
                platform: lxcup_agent::AgentPlatform::Linux,
                hostname: "removal-test".to_owned(),
                version: "0.4.0".to_owned(),
                protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
            },
            metrics: lxcup_agent::AgentMetrics {
                collected_at: Utc::now(),
                commands_total: 0,
                commands_failed: 0,
                last_command_at: None,
            },
            sent_at: Utc::now(),
            telemetry: Default::default(),
            docker_telemetry: Default::default(),
            package_inventory: None,
        },
    );

    let forbidden = delete_target(
        State(state.clone()),
        Extension(ActorRole::Operator),
        Path(target.id.as_uuid().to_string()),
        JsonBody(DeleteTargetRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    let unconfirmed = delete_target(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(target.id.as_uuid().to_string()),
        JsonBody(DeleteTargetRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(unconfirmed.code, "confirmation_required");

    let status = delete_target(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(target.id.as_uuid().to_string()),
        JsonBody(DeleteTargetRequest { confirmed: true }),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::NO_CONTENT);
    let store = state.store.read().await;
    assert!(store.targets.is_empty());
    assert!(!store.agent_reports.contains_key(&target.id));
    assert!(store.update_policies.is_empty());
}

#[tokio::test]
async fn target_removal_is_rejected_while_a_workflow_is_active() {
    let state = ApiState::new();
    let mut target = test_target();
    target.mark_managed();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let request = lxcup_ansible::AnsibleJobRequest {
        operation: lxcup_ansible::AnsibleOperation::HealthCheck,
        target: lxcup_core::ResourceTarget::Target(target_id),
        lifecycle: lxcup_core::ResourceLifecycle::Managed,
        mode: lxcup_ansible::ExecutionMode::Check,
        parameters: lxcup_ansible::AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "remove-target-active-job".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    };
    let job = match state.ansible.write().await.submit(request).unwrap() {
        lxcup_ansible::JobSubmission::Created(job) => job,
        lxcup_ansible::JobSubmission::Duplicate(_) => unreachable!(),
    };
    state
        .ansible
        .write()
        .await
        .transition(job.id, lxcup_ansible::AnsibleJobStatus::Checking)
        .unwrap();

    let error = delete_target(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(target_id.as_uuid().to_string()),
        JsonBody(DeleteTargetRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "target_busy");
    assert_eq!(state.store.read().await.targets.len(), 1);
}

#[tokio::test]
async fn creating_a_target_also_creates_its_standard_update_policy() {
    let state = ApiState::new();
    let request = CreateTargetRequest {
        name: "onboarding-target".to_owned(),
        kind: TargetKind::LinuxServer,
        address: "192.0.2.80".to_owned(),
        transport: TargetTransport::Ssh,
        ssh_user: Some("lxcup".to_owned()),
        credential_secret_ref: Some(SecretId::new()),
        ssh_known_hosts_secret_ref: Some(SecretId::new()),
        agent_secret_ref: SecretId::new(),
    };

    let (_, response) = create_target(
        State(state.clone()),
        Extension(ActorRole::Admin),
        JsonBody(request),
    )
    .await
    .unwrap();

    let target_id = response.0.data.id;
    let policies = state.store.read().await.update_policies.clone();
    assert_eq!(
        policies,
        vec![super::super::default_update_policy(vec![target_id])]
    );
}

#[tokio::test]
async fn windows_target_registration_uses_agent_transport_without_deployment_credentials() {
    let state = ApiState::new();
    let request = CreateTargetRequest {
        name: "windows-agent-target".to_owned(),
        kind: TargetKind::WindowsServer,
        address: "192.0.2.91".to_owned(),
        transport: TargetTransport::Agent,
        ssh_user: None,
        credential_secret_ref: None,
        ssh_known_hosts_secret_ref: None,
        agent_secret_ref: SecretId::new(),
    };

    let (_, response) = create_target(State(state), Extension(ActorRole::Admin), JsonBody(request))
        .await
        .unwrap();

    assert_eq!(response.0.data.kind, TargetKind::WindowsServer);
    assert_eq!(response.0.data.transport, TargetTransport::Agent);
    assert!(response.0.data.credential_secret_ref.is_none());
}

#[tokio::test]
async fn target_registration_rejects_agent_transport_for_non_windows_targets() {
    let state = ApiState::new();
    let request = CreateTargetRequest {
        name: "linux-agent-target".to_owned(),
        kind: TargetKind::LinuxServer,
        address: "192.0.2.92".to_owned(),
        transport: TargetTransport::Agent,
        ssh_user: None,
        credential_secret_ref: None,
        ssh_known_hosts_secret_ref: None,
        agent_secret_ref: SecretId::new(),
    };

    let error = create_target(State(state), Extension(ActorRole::Admin), JsonBody(request))
        .await
        .unwrap_err();

    assert_eq!(error.code, "invalid_target");
}

#[tokio::test]
async fn target_registration_requires_credentials_for_ssh_targets() {
    let state = ApiState::new();
    let request = CreateTargetRequest {
        name: "missing-credential-target".to_owned(),
        kind: TargetKind::LinuxServer,
        address: "192.0.2.93".to_owned(),
        transport: TargetTransport::Ssh,
        ssh_user: None,
        credential_secret_ref: None,
        ssh_known_hosts_secret_ref: None,
        agent_secret_ref: SecretId::new(),
    };

    let error = create_target(State(state), Extension(ActorRole::Admin), JsonBody(request))
        .await
        .unwrap_err();

    assert_eq!(error.code, "invalid_target");
}

#[tokio::test]
async fn get_target_rejects_malformed_and_unknown_ids() {
    let state = ApiState::new();

    let malformed = get_target(State(state.clone()), Path("not-a-uuid".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(malformed.code, "invalid_id");

    let unknown = get_target(State(state), Path(Uuid::new_v4().to_string()))
        .await
        .unwrap_err();
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}
