use super::{
    AGENT_PORT, DiscoverTargetDockerRequest, DockerImageUpdateApplyRequest, DockerLifecycleRequest,
    apply_target_docker_image_update, check_target_docker_image_update, compose_service,
    discover_target_docker, get_target_docker_inventory, is_private_network_address,
    is_safe_image_id, load_target_docker_inventory, map_docker_update_apply_error,
    operate_target_docker_container, resolve_private_agent_address, save_target_docker_inventory,
    validate_fresh_image_update_check,
};
use crate::{ActorRole, ApiState};
use axum::http::StatusCode;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use chrono::Utc;
use lxcup_agent::DockerContainerInfo;
use lxcup_core::TargetId;
use lxcup_persistence::PersistedTargetDockerInventory;
use std::net::IpAddr;

#[test]
fn image_update_apply_requires_a_fresh_matching_check() {
    let check = lxcup_agent::DockerImageUpdateResult {
        container_id: "aaaaaaaaaaaa".to_owned(),
        image: "nginx:stable".to_owned(),
        current_image_id: Some(format!("sha256:{}", "a".repeat(64))),
        remote_image_id: Some(format!("sha256:{}", "b".repeat(64))),
        status: lxcup_agent::DockerImageUpdateStatus::UpdateAvailable,
        reason: None,
        checked_at: chrono::Utc::now(),
    };
    let expected = check.remote_image_id.as_deref().unwrap();
    assert!(validate_fresh_image_update_check(&check, expected).is_ok());
    assert_eq!(
        validate_fresh_image_update_check(&check, "sha256:stale")
            .unwrap_err()
            .code,
        "image_update_check_stale"
    );

    for status in [
        lxcup_agent::DockerImageUpdateStatus::Current,
        lxcup_agent::DockerImageUpdateStatus::Pinned,
        lxcup_agent::DockerImageUpdateStatus::Unknown,
    ] {
        let mut stale = check.clone();
        stale.status = status;
        assert!(validate_fresh_image_update_check(&stale, expected).is_err());
    }
}

#[test]
fn docker_compose_and_image_update_values_are_validated() {
    assert_eq!(
        compose_service(&[
            "com.docker.compose.project=shop".to_owned(),
            "com.docker.compose.service=web".to_owned(),
        ]),
        Some("web")
    );
    assert_eq!(
        compose_service(&["com.docker.compose.service=".to_owned()]),
        None
    );
    assert!(is_safe_image_id(&format!("sha256:{}", "a".repeat(64))));
    assert!(!is_safe_image_id("sha256:abc"));
    assert!(!is_safe_image_id(&format!("sha256:{}", "g".repeat(64))));
}

#[test]
fn docker_apply_errors_map_to_safe_status_codes() {
    let cases = [
        ("container_unavailable", StatusCode::NOT_FOUND),
        ("compose_metadata_required", StatusCode::CONFLICT),
        ("compose_replicas_unsupported", StatusCode::CONFLICT),
        ("compose_config_changed", StatusCode::CONFLICT),
        ("compose_config_hash_required", StatusCode::CONFLICT),
        ("compose_service_scope_changed", StatusCode::CONFLICT),
        ("image_update_check_stale", StatusCode::CONFLICT),
        ("pulled_image_digest_mismatch", StatusCode::CONFLICT),
        ("unknown_failure", StatusCode::BAD_GATEWAY),
    ];
    for (remote_code, status) in cases {
        let error = lxcup_agent::AgentError::Api {
            status: StatusCode::BAD_REQUEST,
            message: serde_json::json!({"error":remote_code}).to_string(),
        };
        assert_eq!(map_docker_update_apply_error(error).status, status);
    }
    let non_api = lxcup_agent::AgentError::InvalidRequest("invalid");
    assert_eq!(
        map_docker_update_apply_error(non_api).status,
        StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn in_memory_target_docker_inventory_is_available_after_discovery() {
    let state = ApiState::new();
    let target_id = TargetId::from_uuid(uuid::Uuid::new_v4());
    let mut inventory = PersistedTargetDockerInventory {
        collected_at: Utc::now(),
        containers: vec![DockerContainerInfo {
            id: "container-1".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: None,
            created_at: None,
            image_id: None,
            restart_count: None,
            health: None,
            oom_killed: None,
            labels: Vec::new(),
        }],
        events: Vec::new(),
    };

    save_target_docker_inventory(&state, target_id, &mut inventory)
        .await
        .unwrap();

    let loaded = load_target_docker_inventory(&state, target_id)
        .await
        .unwrap()
        .expect("a successful discovery should remain available in memory");
    assert_eq!(loaded.containers, inventory.containers);
    assert_eq!(loaded.collected_at, inventory.collected_at);

    let mut updated = inventory.clone();
    updated.collected_at += chrono::Duration::seconds(30);
    updated.containers[0].image = "nginx:2".to_owned();
    updated.containers.push(DockerContainerInfo {
        id: "container-2".to_owned(),
        name: "redis".to_owned(),
        image: "redis:7".to_owned(),
        state: "running".to_owned(),
        status: "Up".to_owned(),
        ports: Vec::new(),
        started_at: None,
        created_at: None,
        image_id: None,
        restart_count: None,
        health: None,
        oom_killed: None,
        labels: Vec::new(),
    });
    save_target_docker_inventory(&state, target_id, &mut updated)
        .await
        .unwrap();
    let loaded = load_target_docker_inventory(&state, target_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        loaded.events.iter().any(|event| {
            event.kind == lxcup_persistence::DockerInventoryEventKind::ImageChanged
        })
    );
    assert!(
        loaded
            .events
            .iter()
            .any(|event| { event.kind == lxcup_persistence::DockerInventoryEventKind::Added })
    );
}

#[test]
fn private_network_filter_rejects_public_and_accepts_local_addresses() {
    for address in [
        "10.0.0.1",
        "192.168.1.20",
        "127.0.0.1",
        "169.254.2.4",
        "fd00::20",
        "fe80::1",
    ] {
        assert!(is_private_network_address(
            address.parse::<IpAddr>().unwrap()
        ));
    }
    for address in ["8.8.8.8", "203.0.113.9", "2001:4860:4860::8888"] {
        assert!(!is_private_network_address(
            address.parse::<IpAddr>().unwrap()
        ));
    }
}

#[tokio::test]
async fn agent_resolution_pins_private_ip_and_rejects_public_ip() {
    let resolved = resolve_private_agent_address("192.168.1.20").await.unwrap();
    assert_eq!(resolved.to_string(), format!("192.168.1.20:{AGENT_PORT}"));
    let rejected = resolve_private_agent_address("203.0.113.9")
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "agent_address_not_private");
}

#[tokio::test]
async fn docker_routes_reject_unsafe_requests_before_agent_access() {
    let state = ApiState::new();
    let discovery = discover_target_docker(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path("invalid".to_owned()),
        Json(DiscoverTargetDockerRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(discovery.code, "confirmation_required");
    let discovery = discover_target_docker(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path("invalid".to_owned()),
        Json(DiscoverTargetDockerRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(discovery.code, "invalid_id");

    let lifecycle = operate_target_docker_container(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path((uuid::Uuid::new_v4().to_string(), "bad-id".to_owned())),
        Json(DockerLifecycleRequest {
            action: lxcup_agent::DockerLifecycleAction::Start,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(lifecycle.code, "confirmation_required");
    let lifecycle = operate_target_docker_container(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path((uuid::Uuid::new_v4().to_string(), "bad-id".to_owned())),
        Json(DockerLifecycleRequest {
            action: lxcup_agent::DockerLifecycleAction::Start,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(lifecycle.code, "invalid_container_id");

    let check = check_target_docker_image_update(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path((uuid::Uuid::new_v4().to_string(), "bad-id".to_owned())),
    )
    .await
    .unwrap_err();
    assert_eq!(check.code, "invalid_container_id");

    let apply = apply_target_docker_image_update(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Path((uuid::Uuid::new_v4().to_string(), "bad-id".to_owned())),
        Json(DockerImageUpdateApplyRequest {
            confirmed: false,
            expected_remote_image_id: "bad-digest".to_owned(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(apply.code, "confirmation_required");
    let apply = apply_target_docker_image_update(
        State(state),
        Extension(ActorRole::Admin),
        Path((uuid::Uuid::new_v4().to_string(), "bad-id".to_owned())),
        Json(DockerImageUpdateApplyRequest {
            confirmed: true,
            expected_remote_image_id: "bad-digest".to_owned(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(apply.code, "invalid_update_request");
}

#[tokio::test]
async fn docker_inventory_endpoint_distinguishes_unknown_and_not_discovered_targets() {
    let state = ApiState::new();
    let target = lxcup_core::Target::new(
        "inventory-host",
        lxcup_core::TargetKind::Lxc,
        "192.168.1.20",
        lxcup_core::TargetTransport::Ssh,
        lxcup_core::SecretId::new(),
        lxcup_core::SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let missing = get_target_docker_inventory(
        State(state.clone()),
        Path(TargetId::new().as_uuid().to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "not_found");
    state.store.write().await.targets.push(target);
    let response = get_target_docker_inventory(State(state), Path(target_id.as_uuid().to_string()))
        .await
        .unwrap()
        .0;
    assert!(!response.data.available);
    assert_eq!(response.data.reason.as_deref(), Some("not_discovered"));
    assert!(response.data.containers.is_empty());
}
