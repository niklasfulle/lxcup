use super::{
    ActorRole, AgentClient, AgentClientConfig, ApiEnvelope, ApiError, ApiState, Json, JsonBody,
    Path, Permission, State, Target, TargetId, TargetKind, TargetState, envelope, map_secret_error,
    parse_uuid, require_permission,
};
use axum::extract::Extension;
use lxcup_agent::DockerContainerInfo;
use lxcup_persistence::{
    DockerInventoryEvent, PersistedTargetDockerInventory, update_docker_inventory_events,
};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

const AGENT_PORT: u16 = 8090;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct DiscoverTargetDockerRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct DockerLifecycleRequest {
    pub action: lxcup_agent::DockerLifecycleAction,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct DockerImageUpdateApplyRequest {
    pub confirmed: bool,
    pub expected_remote_image_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDockerDiscoveryDto {
    pub target_id: TargetId,
    pub target_name: String,
    pub available: bool,
    pub reason: Option<String>,
    pub collected_at: chrono::DateTime<chrono::Utc>,
    pub containers: Vec<DockerContainerInfo>,
    pub events: Vec<DockerInventoryEvent>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDockerLifecycleDto {
    pub target_id: TargetId,
    pub target_name: String,
    pub container_id: String,
    pub container_name: String,
    pub action: lxcup_agent::DockerLifecycleAction,
    pub completed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDockerImageUpdateDto {
    pub target_id: TargetId,
    pub target_name: String,
    pub result: lxcup_agent::DockerImageUpdateResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDockerImageUpdateApplyDto {
    pub target_id: TargetId,
    pub target_name: String,
    pub result: lxcup_agent::DockerImageUpdateApplyResult,
}

pub(super) async fn discover_target_docker(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(target_id): Path<String>,
    JsonBody(request): JsonBody<DiscoverTargetDockerRequest>,
) -> Result<Json<ApiEnvelope<TargetDockerDiscoveryDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Docker discovery must be explicitly confirmed",
        ));
    }
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let (target, client) = connected_lxc_agent(&state, target_id).await?;
    let discovery = client.docker_containers().await.map_err(|_| {
        ApiError::dependency("agent_unreachable", "the LXC agent could not be reached")
    })?;

    let events = if discovery.available {
        let mut inventory = PersistedTargetDockerInventory {
            collected_at: discovery.collected_at,
            containers: discovery.containers.clone(),
            events: Vec::new(),
        };
        save_target_docker_inventory(&state, target_id, &mut inventory).await?;
        inventory.events
    } else {
        Vec::new()
    };

    Ok(Json(envelope(TargetDockerDiscoveryDto {
        target_id,
        target_name: target.name,
        available: discovery.available,
        reason: discovery.reason,
        collected_at: discovery.collected_at,
        containers: discovery.containers,
        events,
    })))
}

pub(super) async fn operate_target_docker_container(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path((target_id, container_id)): Path<(String, String)>,
    JsonBody(request): JsonBody<DockerLifecycleRequest>,
) -> Result<Json<ApiEnvelope<TargetDockerLifecycleDto>>, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Docker lifecycle actions require explicit confirmation",
        ));
    }
    if !lxcup_agent::is_safe_docker_container_id(&container_id) {
        return Err(ApiError::bad_request(
            "invalid_container_id",
            "Docker container id is invalid",
        ));
    }
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let (target, client) = connected_lxc_agent(&state, target_id).await?;
    let discovery = client.docker_containers().await.map_err(|_| {
        ApiError::dependency("agent_unreachable", "the LXC agent could not be reached")
    })?;
    let container = discovery
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .ok_or_else(|| {
            ApiError::not_found("Docker container was not found in the latest agent inventory")
        })?;
    let result = client
        .docker_lifecycle(&lxcup_agent::DockerLifecycleRequest {
            container_id: container_id.clone(),
            action: request.action,
        })
        .await
        .map_err(|_| {
            ApiError::dependency(
                "docker_action_failed",
                "the Docker action failed on the agent",
            )
        })?;
    if let Ok(refreshed) = client.docker_containers().await {
        if refreshed.available {
            let mut inventory = PersistedTargetDockerInventory {
                collected_at: refreshed.collected_at,
                containers: refreshed.containers,
                events: Vec::new(),
            };
            if save_target_docker_inventory(&state, target_id, &mut inventory)
                .await
                .is_err()
            {
                tracing::warn!(target_id = %target_id.as_uuid(), "Docker action succeeded but refreshed inventory could not be persisted");
            }
        }
    }
    Ok(Json(envelope(TargetDockerLifecycleDto {
        target_id,
        target_name: target.name,
        container_id,
        container_name: container.name.clone(),
        action: result.action,
        completed_at: result.completed_at,
    })))
}

pub(super) async fn check_target_docker_image_update(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path((target_id, container_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<TargetDockerImageUpdateDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if !lxcup_agent::is_safe_docker_container_id(&container_id) {
        return Err(ApiError::bad_request(
            "invalid_container_id",
            "Docker container id is invalid",
        ));
    }
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let (target, client) = connected_lxc_agent(&state, target_id).await?;
    let discovery = client.docker_containers().await.map_err(|_| {
        ApiError::dependency("agent_unreachable", "the LXC agent could not be reached")
    })?;
    if !discovery.available
        || !discovery
            .containers
            .iter()
            .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found(
            "Docker container was not found in the latest agent inventory",
        ));
    }
    let result = client
        .docker_image_update_check(&lxcup_agent::DockerImageUpdateRequest { container_id })
        .await
        .map_err(|_| {
            ApiError::dependency(
                "docker_update_check_failed",
                "the agent could not check the image registry",
            )
        })?;
    Ok(Json(envelope(TargetDockerImageUpdateDto {
        target_id,
        target_name: target.name,
        result,
    })))
}

pub(super) async fn apply_target_docker_image_update(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path((target_id, container_id)): Path<(String, String)>,
    JsonBody(request): JsonBody<DockerImageUpdateApplyRequest>,
) -> Result<Json<ApiEnvelope<TargetDockerImageUpdateApplyDto>>, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Docker image updates require explicit confirmation",
        ));
    }
    if !lxcup_agent::is_safe_docker_container_id(&container_id)
        || !is_safe_image_id(&request.expected_remote_image_id)
    {
        return Err(ApiError::bad_request(
            "invalid_update_request",
            "Docker image update request is invalid",
        ));
    }
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let (target, client) = connected_lxc_agent(&state, target_id).await?;
    let discovery = client.docker_containers().await.map_err(|_| {
        ApiError::dependency("agent_unreachable", "the LXC agent could not be reached")
    })?;
    let container = discovery
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .ok_or_else(|| {
            ApiError::not_found("Docker container was not found in the latest inventory")
        })?;
    if compose_service(&container.labels).is_none() {
        return Err(ApiError::conflict(
            "compose_metadata_required",
            "image updates are supported only for Compose-managed containers",
        ));
    }
    let check = client
        .docker_image_update_check(&lxcup_agent::DockerImageUpdateRequest {
            container_id: container_id.clone(),
        })
        .await
        .map_err(|_| {
            ApiError::dependency(
                "docker_update_check_failed",
                "the agent could not check the image registry",
            )
        })?;
    validate_fresh_image_update_check(&check, &request.expected_remote_image_id)?;
    let result = client
        .docker_image_update_apply(&lxcup_agent::DockerImageUpdateApplyRequest {
            container_id: container_id.clone(),
            expected_remote_image_id: request.expected_remote_image_id,
        })
        .await
        .map_err(map_docker_update_apply_error)?;
    if let Some(repositories) = state.repositories.as_ref() {
        let audit = repositories
            .audit_events
            .append(&lxcup_persistence::AuditEvent {
                id: uuid::Uuid::new_v4(),
                node_id: None,
                container_id: None,
                plan_id: None,
                execution_id: None,
                event_type: "docker.image_update.applied".to_owned(),
                details: serde_json::json!({
                    "target_id": target_id.as_uuid(),
                    "docker_container_id": container_id,
                    "compose_project": result.compose_project,
                    "compose_service": result.compose_service,
                    "previous_image_id": container.image_id,
                    "image_id": result.image_id,
                    "actor_role": format!("{actor_role:?}").to_lowercase(),
                }),
                created_at: result.completed_at,
            })
            .await;
        if let Err(error) = audit {
            tracing::error!(target_id = %target_id.as_uuid(), error = ?error, "Docker image update succeeded but its audit event could not be stored");
        }
    }
    state.publish(crate::ApiEvent::status(
        "docker-container",
        container_id.clone(),
        "image_updated",
    ));
    if let Ok(refreshed) = client.docker_containers().await {
        if refreshed.available {
            let mut inventory = PersistedTargetDockerInventory {
                collected_at: refreshed.collected_at,
                containers: refreshed.containers,
                events: Vec::new(),
            };
            if save_target_docker_inventory(&state, target_id, &mut inventory)
                .await
                .is_err()
            {
                tracing::warn!(target_id = %target_id.as_uuid(), "Docker image update succeeded but refreshed inventory could not be persisted");
            }
        }
    }
    Ok(Json(envelope(TargetDockerImageUpdateApplyDto {
        target_id,
        target_name: target.name,
        result,
    })))
}

fn compose_service(labels: &[String]) -> Option<&str> {
    labels.iter().find_map(|label| {
        label
            .strip_prefix("com.docker.compose.service=")
            .filter(|service| !service.is_empty())
    })
}

fn is_safe_image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn validate_fresh_image_update_check(
    check: &lxcup_agent::DockerImageUpdateResult,
    expected_remote_image_id: &str,
) -> Result<(), ApiError> {
    if check.status == lxcup_agent::DockerImageUpdateStatus::UpdateAvailable
        && check.remote_image_id.as_deref() == Some(expected_remote_image_id)
    {
        return Ok(());
    }
    Err(ApiError::conflict(
        "image_update_check_stale",
        "a current successful image update check is required before applying",
    ))
}

fn map_docker_update_apply_error(error: lxcup_agent::AgentError) -> ApiError {
    let remote_code = match &error {
        lxcup_agent::AgentError::Api { message, .. } => {
            serde_json::from_str::<serde_json::Value>(message)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
        }
        _ => None,
    };
    match remote_code.as_deref() {
        Some("container_unavailable") => {
            ApiError::not_found("Docker container is no longer available")
        }
        Some("compose_metadata_required") => ApiError::conflict(
            "compose_metadata_required",
            "image updates are supported only for Compose-managed containers",
        ),
        Some("compose_replicas_unsupported") => ApiError::conflict(
            "compose_replicas_unsupported",
            "only a single-replica Compose service can be updated",
        ),
        Some("compose_config_hash_required" | "compose_config_changed") => ApiError::conflict(
            "compose_config_changed",
            "the Compose service configuration changed; rediscover and review it before updating",
        ),
        Some("compose_service_scope_changed") => ApiError::conflict(
            "compose_service_scope_changed",
            "the Compose service now manages a different set of containers; rediscover before updating",
        ),
        Some("image_update_check_stale" | "pulled_image_digest_mismatch") => ApiError::conflict(
            "image_update_check_stale",
            "the remote image changed since it was checked; run the update check again",
        ),
        _ => ApiError::dependency(
            "docker_update_apply_failed",
            "the Compose image update failed on the agent",
        ),
    }
}

async fn connected_lxc_agent(
    state: &ApiState,
    target_id: TargetId,
) -> Result<(Target, AgentClient), ApiError> {
    let target = {
        let store = state.store.read().await;
        store
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?
    };
    if target.kind != TargetKind::Lxc || target.state != TargetState::Managed {
        return Err(ApiError::conflict(
            "target_not_ready",
            "Docker actions require a connected LXC target",
        ));
    }
    let address = resolve_private_agent_address(&target.address).await?;
    let token = state
        .secrets
        .read(target.agent_secret_ref)
        .map_err(map_secret_error)?;
    let config = AgentClientConfig::new_for_private_network_http(
        format!("http://{address}"),
        token.expose(),
    )
    .map_err(|_| ApiError::bad_request("invalid_agent_address", "agent address is invalid"))?;
    let client = AgentClient::new(config).map_err(|_| {
        ApiError::dependency(
            "agent_client_error",
            "the agent client could not be created",
        )
    })?;
    Ok((target, client))
}

pub(super) async fn get_target_docker_inventory(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<TargetDockerDiscoveryDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let target = {
        let store = state.store.read().await;
        store
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?
    };
    let inventory = load_target_docker_inventory(&state, target_id).await?;
    let (available, reason, collected_at, containers, events) = match inventory {
        Some(inventory) => (
            true,
            None,
            inventory.collected_at,
            inventory.containers,
            inventory.events,
        ),
        None => (
            false,
            Some("not_discovered".to_owned()),
            chrono::Utc::now(),
            Vec::new(),
            Vec::new(),
        ),
    };
    Ok(Json(envelope(TargetDockerDiscoveryDto {
        target_id,
        target_name: target.name,
        available,
        reason,
        collected_at,
        containers,
        events,
    })))
}

async fn save_target_docker_inventory(
    state: &ApiState,
    target_id: TargetId,
    inventory: &mut PersistedTargetDockerInventory,
) -> Result<(), ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .target_docker_inventory
            .save(target_id, inventory)
            .await
            .map_err(|_| ApiError::storage())?;
    } else {
        let mut store = state.store.write().await;
        let previous = store.target_docker_inventories.get(&target_id).cloned();
        update_docker_inventory_events(previous.as_ref(), inventory);
        store
            .target_docker_inventories
            .insert(target_id, inventory.clone());
    }
    Ok(())
}

async fn load_target_docker_inventory(
    state: &ApiState,
    target_id: TargetId,
) -> Result<Option<PersistedTargetDockerInventory>, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .target_docker_inventory
            .get(target_id)
            .await
            .map_err(|_| ApiError::storage())
    } else {
        Ok(state
            .store
            .read()
            .await
            .target_docker_inventories
            .get(&target_id)
            .cloned())
    }
}

async fn resolve_private_agent_address(address: &str) -> Result<SocketAddr, ApiError> {
    let mut resolved = tokio::net::lookup_host((address, AGENT_PORT))
        .await
        .map_err(|_| ApiError::bad_request("invalid_agent_address", "agent address is invalid"))?;
    resolved
        .find(|address| is_private_network_address(address.ip()))
        .ok_or_else(|| {
            ApiError::bad_request(
                "agent_address_not_private",
                "Docker discovery is restricted to private network addresses",
            )
        })
}

fn is_private_network_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() || address.is_loopback() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PORT, compose_service, is_private_network_address, is_safe_image_id,
        load_target_docker_inventory, map_docker_update_apply_error, resolve_private_agent_address,
        save_target_docker_inventory, validate_fresh_image_update_check,
    };
    use crate::ApiState;
    use axum::http::StatusCode;
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
        assert!(loaded.events.iter().any(|event| {
            event.kind == lxcup_persistence::DockerInventoryEventKind::ImageChanged
        }));
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
}
