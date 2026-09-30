use super::*;
use axum::{Json, routing::get};
use lxcup_secrets::{CreateSecret, SecretStore};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;

fn secret_request(name: &str, kind: SecretKind, value: &str) -> CreateSecretRequest {
    CreateSecretRequest {
        name: name.to_owned(),
        kind,
        scope: SecretScope::Global,
        value: value.to_owned(),
    }
}

fn docker_info(name: &str, image: &str) -> DockerContainerInfo {
    DockerContainerInfo {
        id: "docker-1".to_owned(),
        name: name.to_owned(),
        image: image.to_owned(),
        state: "running".to_owned(),
        status: "Up 10 seconds".to_owned(),
        ports: vec!["8080/tcp".to_owned()],
        started_at: Some("2026-01-01T00:00:00Z".to_owned()),
        created_at: None,
        image_id: None,
        restart_count: None,
        health: None,
        oom_killed: None,
        labels: vec!["app=web".to_owned()],
    }
}

fn docker_result(containers: Vec<DockerContainerInfo>) -> lxcup_agent::DockerDiscovery {
    lxcup_agent::DockerDiscovery {
        available: true,
        reason: None,
        collected_at: chrono::Utc::now(),
        containers,
    }
}

async fn next_docker_result(
    State(queue): State<Arc<AsyncMutex<VecDeque<lxcup_agent::DockerDiscovery>>>>,
) -> Json<lxcup_agent::DockerDiscovery> {
    Json(queue.lock().await.pop_front().expect("test result queued"))
}

#[derive(Clone)]
struct DiscoveryConcurrency {
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
}

async fn slow_docker_result(
    State(counter): State<DiscoveryConcurrency>,
) -> Json<lxcup_agent::DockerDiscovery> {
    let active = counter.active.fetch_add(1, Ordering::SeqCst) + 1;
    counter.maximum.fetch_max(active, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    counter.active.fetch_sub(1, Ordering::SeqCst);
    Json(docker_result(vec![docker_info("web", "nginx:1")]))
}

#[test]
fn secret_store_errors_map_to_stable_api_categories() {
    let cases = [
        (
            SecretStoreError::Missing,
            StatusCode::BAD_GATEWAY,
            "agent_secret_missing",
        ),
        (
            SecretStoreError::Denied,
            StatusCode::FORBIDDEN,
            "agent_secret_denied",
        ),
        (
            SecretStoreError::Invalid,
            StatusCode::BAD_REQUEST,
            "agent_secret_invalid",
        ),
        (
            SecretStoreError::Unavailable,
            StatusCode::BAD_GATEWAY,
            "secret_store_unavailable",
        ),
    ];
    for (error, status, code) in cases {
        let mapped = map_secret_error(error);
        assert_eq!(mapped.status, status);
        assert_eq!(mapped.code, code);
        assert!(!mapped.message.is_empty());
    }
}

#[test]
fn secret_dtos_never_contain_secret_material() {
    let error = map_secret_error(SecretStoreError::Invalid);
    let rendered = format!("{error:?}");
    assert!(!rendered.contains("secret value"));
}

#[test]
fn docker_workload_dto_preserves_discovered_and_managed_states() {
    let discovered = DockerWorkloadDto::discovered(
        ContainerId::new(101),
        DockerContainerInfo {
            id: "docker-id".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: Some("2026-01-01T00:00:00Z".to_owned()),
            created_at: None,
            image_id: None,
            restart_count: None,
            health: None,
            oom_killed: None,
            labels: vec!["app=web".to_owned()],
        },
    );
    assert_eq!(discovered.management_state, "discovered");
    let managed = DockerWorkload {
        host_container_id: ContainerId::new(101),
        id: discovered.id.clone(),
        name: discovered.name.clone(),
        image: discovered.image.clone(),
        state: discovered.state.clone(),
        status: discovered.status.clone(),
        ports: discovered.ports.clone(),
        started_at: discovered.started_at.clone(),
        labels: discovered.labels.clone(),
        presence: lxcup_core::DockerWorkloadPresence::Present,
        change: lxcup_core::DockerWorkloadChange::Changed,
        management_state: DockerWorkloadManagementState::Managed,
        discovered_at: discovered.discovered_at,
    };
    assert_eq!(DockerWorkloadDto::from(managed).management_state, "managed");
}

#[tokio::test]
async fn secret_handlers_cover_list_get_rotate_revoke_delete_and_audit() {
    let state = ApiState::new();
    let (_, Json(created)) = create_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Json(secret_request(
            "primary",
            SecretKind::Generic,
            "first-value",
        )),
    )
    .await
    .unwrap();
    let id = created.data.metadata.metadata.id;

    let Json(listed) = list_secrets(State(state.clone())).await.unwrap();
    assert_eq!(listed.data.len(), 1);
    assert_eq!(listed.data[0].metadata.metadata.name, "primary");
    let Json(found) = get_secret(State(state.clone()), Path(id.as_uuid().to_string()))
        .await
        .unwrap();
    assert_eq!(found.data.metadata.metadata.id, id);
    assert!(
        get_secret(State(state.clone()), Path("bad-id".to_owned()))
            .await
            .is_err()
    );

    let no_confirmation = rotate_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(id.as_uuid().to_string()),
        Json(SecretValueRequest {
            value: "ignored".to_owned(),
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(no_confirmation.code, "confirmation_required");
    let Json(rotated) = rotate_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(id.as_uuid().to_string()),
        Json(SecretValueRequest {
            value: "rotated-value".to_owned(),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(rotated.data.metadata.metadata.id, id);

    let revoke_without_confirmation = revoke_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(id.as_uuid().to_string()),
        Json(SecretConfirmationRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(revoke_without_confirmation.code, "confirmation_required");
    let _ = revoke_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(id.as_uuid().to_string()),
        Json(SecretConfirmationRequest { confirmed: true }),
    )
    .await
    .unwrap();

    let (_, Json(deletable)) = create_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Json(secret_request(
            "delete-me",
            SecretKind::SshPassword,
            "password",
        )),
    )
    .await
    .unwrap();
    let delete_id = deletable.data.metadata.metadata.id;
    let delete_without_confirmation = delete_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(delete_id.as_uuid().to_string()),
        Json(SecretConfirmationRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(delete_without_confirmation.code, "confirmation_required");
    assert_eq!(
        delete_secret(
            State(state.clone()),
            Extension(ActorRole::Admin),
            Path(delete_id.as_uuid().to_string()),
            Json(SecretConfirmationRequest { confirmed: true }),
        )
        .await
        .unwrap(),
        StatusCode::NO_CONTENT
    );

    let Json(audit) = list_secret_audit(State(state)).await.unwrap();
    let actions = audit
        .data
        .into_iter()
        .map(|event| event.action)
        .collect::<Vec<_>>();
    assert!(actions.contains(&"created".to_owned()));
    assert!(actions.contains(&"rotated".to_owned()));
    assert!(actions.contains(&"revoked".to_owned()));
    assert!(actions.contains(&"deleted".to_owned()));
}

#[tokio::test]
async fn rotating_agent_token_queues_reconfiguration_for_enabled_target() {
    let secret_store = lxcup_secrets::InMemorySecretStore::default();
    let secret = secret_store
        .create(CreateSecret {
            name: "agent-token".to_owned(),
            kind: SecretKind::AgentToken,
            scope: SecretScope::Global,
            value: SecretValue::new("old-token").unwrap(),
        })
        .unwrap();
    let target = lxcup_core::Target::new(
        "agent-token-target",
        lxcup_core::TargetKind::LinuxServer,
        "192.0.2.10",
        lxcup_core::TargetTransport::Ssh,
        SecretId::new(),
        secret.metadata.id,
    )
    .unwrap();
    let target_id = target.id;
    let state = ApiState::new().with_secret_store(Arc::new(secret_store));
    state.store.write().await.targets.push(target);

    let _ = rotate_secret(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(secret.metadata.id.as_uuid().to_string()),
        Json(SecretValueRequest {
            value: "new-token".to_owned(),
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let jobs = state.ansible.read().await.jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].target,
        lxcup_core::ResourceTarget::Target(target_id)
    );
    assert!(
        state
            .store
            .read()
            .await
            .secret_audit
            .iter()
            .any(|event| { event.action == "agent_reconfiguration_queued" })
    );
}

#[tokio::test]
async fn agent_registration_rejects_invalid_permission_container_secret_and_endpoint() {
    let state = ApiState::new();
    let request = || RegisterAgentRequest {
        endpoint: "http://127.0.0.1:1".to_owned(),
        secret_ref: None,
        ca_secret_ref: None,
        token: None,
    };

    let unauthorized = register_agent(
        State(state.clone()),
        Extension(ActorRole::Viewer),
        Path("not-an-id".to_owned()),
        Json(request()),
    )
    .await
    .unwrap_err();
    assert_eq!(unauthorized.code, "permission_denied");

    let malformed = register_agent(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path("not-an-id".to_owned()),
        Json(request()),
    )
    .await
    .unwrap_err();
    assert_eq!(malformed.code, "invalid_id");

    let container_id = ContainerId::new(777);
    let missing_container = register_agent(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(container_id.value().to_string()),
        Json(request()),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_container.code, "not_found");

    state
        .replace_containers(vec![
            lxcup_core::Container::new(
                container_id,
                lxcup_core::NodeId::new(),
                "agent-registration-test",
                lxcup_core::OperatingSystem::Debian,
                lxcup_core::ContainerStatus::Running,
            )
            .unwrap(),
        ])
        .await;
    let missing_secret = register_agent(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path(container_id.value().to_string()),
        Json(request()),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_secret.code, "agent_secret_required");

    let secret = state
        .secrets
        .create(CreateSecret {
            name: "registration-token".to_owned(),
            kind: SecretKind::AgentToken,
            scope: SecretScope::Global,
            value: SecretValue::new("test-token").unwrap(),
        })
        .unwrap();
    let invalid_endpoint = register_agent(
        State(state),
        Extension(ActorRole::Admin),
        Path(container_id.value().to_string()),
        Json(RegisterAgentRequest {
            endpoint: "not-a-url".to_owned(),
            secret_ref: Some(secret.metadata.id),
            ca_secret_ref: None,
            token: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid_endpoint.code, "invalid_agent_config");
}

#[tokio::test]
async fn agent_registration_persists_the_verified_registration_with_repositories() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("agent_registration_api_{}", uuid::Uuid::new_v4().simple());
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
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(10),
        None,
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let node = lxcup_core::Node::new("agent-registration-api", "https://node.invalid").unwrap();
    repositories.nodes.save(&node).await.unwrap();
    let container_id = ContainerId::new(72_004);
    let container = lxcup_core::Container::new(
        container_id,
        node.id,
        "agent-registration-api-host",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    repositories.containers.save(&container).await.unwrap();

    let token = "registration-api-test-token";
    let info = lxcup_agent::AgentInfo {
        agent_id: "registration-api-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Linux,
        hostname: "registration-api-host".to_owned(),
        version: lxcup_core::VERSION.to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(info, token)),
        )
        .await
        .unwrap();
    });

    let secret_store = lxcup_secrets::InMemorySecretStore::default();
    let secret = secret_store
        .create(CreateSecret {
            name: "registration-api-token".to_owned(),
            kind: SecretKind::AgentToken,
            scope: SecretScope::Container(container_id),
            value: SecretValue::new(token).unwrap(),
        })
        .unwrap();
    let state = ApiState::new()
        .with_repositories(repositories.clone())
        .with_secret_store(Arc::new(secret_store));
    state.replace_containers(vec![container]).await;
    let (_, Json(response)) = register_agent(
        State(state),
        Extension(ActorRole::Admin),
        Path(container_id.value().to_string()),
        Json(RegisterAgentRequest {
            endpoint: format!("http://{address}"),
            secret_ref: Some(secret.metadata.id),
            ca_secret_ref: None,
            token: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(response.data.info.info.agent_id, "registration-api-agent");
    let persisted = repositories
        .agent_registrations
        .find_by_container(container_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.agent_id, "registration-api-agent");
    assert_eq!(persisted.secret_ref, secret.metadata.id);

    agent_task.abort();
    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn agent_health_metrics_and_revoke_reject_invalid_or_unregistered_agents() {
    let state = ApiState::new();
    let invalid_health = get_agent_health(State(state.clone()), Path("bad-id".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(invalid_health.code, "invalid_id");
    let missing_health = get_agent_health(State(state.clone()), Path("779".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(missing_health.code, "not_found");

    let invalid_metrics = get_agent_metrics(State(state.clone()), Path("bad-id".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(invalid_metrics.code, "invalid_id");
    let missing_metrics = get_agent_metrics(State(state.clone()), Path("779".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(missing_metrics.code, "not_found");

    let container_id = ContainerId::new(779);
    let client = AgentClient::new(
        AgentClientConfig::new("http://127.0.0.1:1", "unreachable-agent-token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });
    let mut events = state.events.subscribe();
    let unreachable_health =
        get_agent_health(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap_err();
    assert_eq!(unreachable_health.code, "agent_unavailable");
    assert!(
        matches!(events.try_recv(), Ok(ApiEvent::Status { ref state, .. }) if state == "unreachable")
    );
    let unreachable_metrics =
        get_agent_metrics(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap_err();
    assert_eq!(unreachable_metrics.code, "agent_unavailable");

    let denied = revoke_agent(
        State(state.clone()),
        Extension(ActorRole::Viewer),
        Path("bad-id".to_owned()),
        Json(ConfirmedRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.code, "permission_denied");
    let unconfirmed = revoke_agent(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path("bad-id".to_owned()),
        Json(ConfirmedRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(unconfirmed.code, "confirmation_required");
    let revoked = revoke_agent(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path("779".to_owned()),
        Json(ConfirmedRequest { confirmed: true }),
    )
    .await
    .unwrap();
    assert_eq!(revoked, StatusCode::NO_CONTENT);
    let missing_revoke = revoke_agent(
        State(state),
        Extension(ActorRole::Admin),
        Path("779".to_owned()),
        Json(ConfirmedRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_revoke.code, "not_found");
}

#[tokio::test]
async fn docker_discovery_tracks_new_unchanged_changed_and_missing_workloads() {
    let results = Arc::new(AsyncMutex::new(VecDeque::from([
        docker_result(vec![docker_info("web", "nginx:1")]),
        docker_result(vec![docker_info("web", "nginx:1")]),
        docker_result(vec![docker_info("web-v2", "nginx:2")]),
        docker_result(Vec::new()),
    ])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/docker/containers", get(next_docker_result))
                .with_state(results),
        )
        .await
        .unwrap();
    });

    let container_id = ContainerId::new(808);
    let client = AgentClient::new(
        AgentClientConfig::new(format!("http://{address}"), "docker-test-token").unwrap(),
    )
    .unwrap();
    let state = ApiState::new();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });

    let Json(first) =
        discover_docker_containers(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap();
    assert_eq!(first.data.workloads[0].change_state, "new");
    let _ = adopt_docker_container(
        State(state.clone()),
        Path((container_id.value().to_string(), "docker-1".to_owned())),
    )
    .await
    .unwrap();

    let Json(second) =
        discover_docker_containers(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap();
    assert_eq!(second.data.workloads[0].change_state, "unchanged");
    assert_eq!(second.data.workloads[0].management_state, "managed");

    let Json(third) =
        discover_docker_containers(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap();
    assert_eq!(third.data.workloads[0].change_state, "changed");
    assert_eq!(third.data.workloads[0].name, "web-v2");

    let Json(fourth) =
        discover_docker_containers(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap();
    assert_eq!(fourth.data.workloads[0].presence, "missing");
    assert_eq!(fourth.data.workloads[0].change_state, "missing");
    assert_eq!(
        fourth.data.run.status,
        lxcup_core::DockerDiscoveryStatus::Succeeded
    );
    let Json(latest) =
        get_docker_discovery(State(state.clone()), Path(container_id.value().to_string()))
            .await
            .unwrap();
    assert_eq!(latest.data.unwrap().container_count, 0);
    let Json(listed) = list_docker_containers(State(state), Path(container_id.value().to_string()))
        .await
        .unwrap();
    assert_eq!(listed.data.len(), 1);
    agent_task.abort();
}

#[tokio::test]
async fn scheduled_docker_discovery_records_failures_and_later_recovers() {
    let mut unavailable = docker_result(Vec::new());
    unavailable.available = false;
    unavailable.reason = Some("docker unavailable".to_owned());
    let results = Arc::new(AsyncMutex::new(VecDeque::from([
        unavailable,
        docker_result(vec![docker_info("web", "nginx:1")]),
    ])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/docker/containers", get(next_docker_result))
                .with_state(results),
        )
        .await
        .unwrap();
    });

    let state = ApiState::new();
    let mut target = lxcup_core::Target::new(
        "scheduled-lxc",
        lxcup_core::TargetKind::Lxc,
        "127.0.0.1",
        lxcup_core::TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    let container_id = ContainerId::new(810);
    let mut enrollment = lxcup_core::Enrollment::new(container_id, "scheduled-docker").unwrap();
    enrollment.target_id = Some(target_id);
    enrollment.state = lxcup_core::EnrollmentState::Connected;
    state.store.write().await.targets.push(target);
    state.store.write().await.enrollments.push(enrollment);
    state
        .store
        .write()
        .await
        .schedules
        .push(lxcup_core::JobSchedule {
            id: "docker-every-hour".to_owned(),
            operation: "docker_discovery".to_owned(),
            timezone: "UTC".to_owned(),
            target_ids: vec![target_id],
            frequency: lxcup_core::ScheduleFrequency::EveryMinutes(60),
            enabled: true,
            threshold: None,
            policy_id: None,
            last_run_at: None,
            next_run_at: chrono::Utc::now() - chrono::Duration::minutes(1),
            last_error: None,
        });
    let client = AgentClient::new(
        AgentClientConfig::new(format!("http://{address}"), "docker-schedule-token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });

    assert_eq!(state.dispatch_due_schedules().await, 0);
    let first = state.store.read().await;
    assert_eq!(
        first.docker_discovery_runs[0].status,
        lxcup_core::DockerDiscoveryStatus::Failed
    );
    assert!(first.schedules[0].last_error.is_some());
    drop(first);

    state.store.write().await.schedules[0].next_run_at =
        chrono::Utc::now() - chrono::Duration::minutes(1);
    assert_eq!(state.dispatch_due_schedules().await, 1);
    let recovered = state.store.read().await;
    assert_eq!(
        recovered.docker_discovery_runs[1].status,
        lxcup_core::DockerDiscoveryStatus::Succeeded
    );
    assert_eq!(recovered.docker_workloads.len(), 1);
    assert!(recovered.schedules[0].last_error.is_none());
    drop(recovered);
    agent_task.abort();
}

#[tokio::test]
async fn simultaneous_docker_discoveries_are_serialized() {
    let counter = DiscoveryConcurrency {
        active: Arc::new(AtomicUsize::new(0)),
        maximum: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let maximum = counter.maximum.clone();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/docker/containers", get(slow_docker_result))
                .with_state(counter),
        )
        .await
        .unwrap();
    });

    let container_id = ContainerId::new(811);
    let state = ApiState::new();
    let client = AgentClient::new(
        AgentClientConfig::new(format!("http://{address}"), "docker-parallel-token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });
    let first = run_docker_discovery(&state, container_id);
    let second = run_docker_discovery(&state, container_id);
    let (first, second) = tokio::join!(first, second);
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(state.store.read().await.docker_discovery_runs.len(), 2);
    assert_eq!(maximum.load(Ordering::SeqCst), 1);

    agent_task.abort();
}

#[test]
fn docker_change_detection_handles_missing_previous_and_all_state_changes() {
    let current = docker_info("web", "nginx:1");
    assert_eq!(docker_workload_change(None, &current), "new");
    let previous = DockerWorkloadDto::discovered(ContainerId::new(809), current.clone());
    assert_eq!(
        docker_workload_change(Some(&previous), &current),
        "unchanged"
    );

    let mut missing = previous.clone();
    missing.presence = "missing".to_owned();
    assert_eq!(docker_workload_change(Some(&missing), &current), "changed");
    let changed = docker_info("renamed", "nginx:2");
    assert_eq!(docker_workload_change(Some(&previous), &changed), "changed");
}
