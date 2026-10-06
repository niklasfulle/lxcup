use super::*;
use axum::Extension;
use lxcup_core::ActorRole;
use std::time::Duration;
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
async fn windows_heartbeat_keeps_store_and_unmatched_software_in_inventory() {
    use lxcup_secrets::{CreateSecret, InMemorySecretStore, SecretStore};
    use std::sync::Arc;

    let secrets = InMemorySecretStore::default();
    let token = "windows-heartbeat-token";
    let secret = secrets
        .create(CreateSecret {
            name: "windows-heartbeat-agent".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new(token).unwrap(),
        })
        .unwrap();
    let mut target = Target::new_agent_only(
        "windows-heartbeat",
        TargetKind::WindowsServer,
        "192.0.2.100",
        secret.metadata.id,
    )
    .unwrap();
    let target_id = target.id;
    let now = Utc::now();
    let state = ApiState::new().with_secret_store(Arc::new(secrets));
    state.store.write().await.targets.push(target.clone());
    let heartbeat = lxcup_agent::AgentHeartbeat {
        target_id: target_id.as_uuid(),
        info: lxcup_agent::AgentInfo {
            agent_id: "windows-heartbeat-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Windows,
            hostname: "windows-host".to_owned(),
            version: "0.5.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        },
        metrics: lxcup_agent::AgentMetrics {
            collected_at: now,
            commands_total: 0,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: lxcup_agent::SystemTelemetryWindow {
            samples: vec![lxcup_agent::SystemTelemetrySample {
                collected_at: now,
                cpu_basis_points: Some(2_500),
                memory_basis_points: Some(4_000),
                storage_basis_points: Some(5_000),
                load_1_milli: None,
                network_rx_bytes: Some(100),
                network_tx_bytes: Some(200),
                process_count: Some(42),
            }],
            partial: false,
        },
        docker_telemetry: Default::default(),
        package_inventory: Some(lxcup_agent::AgentPackageInventory {
            collected_at: now,
            packages: vec![
                lxcup_agent::AgentInstalledPackage {
                    name: "9N0DX20HK701".to_owned(),
                    installed_version: "14.0.30704.0".to_owned(),
                    candidate_version: None,
                    architecture: None,
                    source: Some("msstore".to_owned()),
                },
                lxcup_agent::AgentInstalledPackage {
                    name: r"ARP\Machine\{12345678-1234-1234-1234-123456789abc}".to_owned(),
                    installed_version: "1.0".to_owned(),
                    candidate_version: Some("2.0".to_owned()),
                    architecture: None,
                    source: Some("arp".to_owned()),
                },
                lxcup_agent::AgentInstalledPackage {
                    name: "Contoso.Unrecognized.Source".to_owned(),
                    installed_version: "1.0".to_owned(),
                    candidate_version: None,
                    architecture: None,
                    source: Some("unknown".to_owned()),
                },
            ],
        }),
    };
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());

    let response = receive_agent_heartbeat(State(state.clone()), headers, JsonBody(heartbeat))
        .await
        .expect("an optional Store package must not reject the whole heartbeat");

    assert_eq!(response, StatusCode::NO_CONTENT);
    let store = state.store.read().await;
    target = store
        .targets
        .iter()
        .find(|item| item.id == target_id)
        .unwrap()
        .clone();
    assert_eq!(target.state, TargetState::Managed);
    let report = store.agent_reports.get(&target_id).unwrap();
    assert_eq!(report.telemetry.samples.len(), 1);
    assert_eq!(report.telemetry.samples[0].cpu_basis_points, Some(2_500));
    let inventory = store.package_inventories.get(&target_id).unwrap();
    assert_eq!(inventory.packages.len(), 2);
    assert_eq!(inventory.packages[0].source.as_deref(), Some("msstore"));
    assert_eq!(inventory.packages[1].source.as_deref(), Some("arp"));
    assert_eq!(inventory.packages[1].candidate_version, None);
}

#[tokio::test]
async fn api_creates_same_named_targets_of_different_kinds_in_postgres() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("duplicate_target_names_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        1,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let state = ApiState::new().with_repositories(repositories.clone());
    let name = format!("duplicate-label-{}", Uuid::new_v4().simple());

    let linux_request = CreateTargetRequest {
        name: name.clone(),
        kind: TargetKind::LinuxServer,
        address: "192.0.2.242".to_owned(),
        transport: TargetTransport::Ssh,
        ssh_user: Some("lxcup".to_owned()),
        credential_secret_ref: Some(SecretId::new()),
        ssh_known_hosts_secret_ref: Some(SecretId::new()),
        agent_secret_ref: SecretId::new(),
    };
    let (_, linux_response) = create_target(
        State(state.clone()),
        Extension(ActorRole::Admin),
        JsonBody(linux_request),
    )
    .await
    .unwrap();

    let windows_request = CreateTargetRequest {
        name,
        kind: TargetKind::WindowsServer,
        address: "192.0.2.243".to_owned(),
        transport: TargetTransport::Agent,
        ssh_user: None,
        credential_secret_ref: None,
        ssh_known_hosts_secret_ref: None,
        agent_secret_ref: SecretId::new(),
    };
    let (_, windows_response) = create_target(
        State(state),
        Extension(ActorRole::Admin),
        JsonBody(windows_request),
    )
    .await
    .expect("the API should allow equal display names for distinct targets");

    let linux = repositories
        .targets
        .find_by_id(linux_response.0.data.id)
        .await
        .unwrap()
        .unwrap();
    let windows = repositories
        .targets
        .find_by_id(windows_response.0.data.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(linux.name, windows.name);
    assert_eq!(linux.kind, TargetKind::LinuxServer);
    assert_eq!(windows.kind, TargetKind::WindowsServer);

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
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
