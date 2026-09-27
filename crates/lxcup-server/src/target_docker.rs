use super::{
    ActorRole, AgentClient, AgentClientConfig, ApiEnvelope, ApiError, ApiState, Json, JsonBody,
    Path, Permission, State, TargetId, TargetKind, TargetState, envelope, map_secret_error,
    parse_uuid, require_permission,
};
use axum::extract::Extension;
use lxcup_agent::DockerContainerInfo;
use lxcup_persistence::PersistedTargetDockerInventory;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

const AGENT_PORT: u16 = 8090;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct DiscoverTargetDockerRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDockerDiscoveryDto {
    pub target_id: TargetId,
    pub target_name: String,
    pub available: bool,
    pub reason: Option<String>,
    pub collected_at: chrono::DateTime<chrono::Utc>,
    pub containers: Vec<DockerContainerInfo>,
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
            "Docker discovery requires a connected LXC target",
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
    let discovery = client.docker_containers().await.map_err(|_| {
        ApiError::dependency("agent_unreachable", "the LXC agent could not be reached")
    })?;

    if discovery.available {
        save_target_docker_inventory(
            &state,
            target_id,
            PersistedTargetDockerInventory {
                collected_at: discovery.collected_at,
                containers: discovery.containers.clone(),
            },
        )
        .await?;
    }

    Ok(Json(envelope(TargetDockerDiscoveryDto {
        target_id,
        target_name: target.name,
        available: discovery.available,
        reason: discovery.reason,
        collected_at: discovery.collected_at,
        containers: discovery.containers,
    })))
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
    let (available, reason, collected_at, containers) = match inventory {
        Some(inventory) => (true, None, inventory.collected_at, inventory.containers),
        None => (
            false,
            Some("not_discovered".to_owned()),
            chrono::Utc::now(),
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
    })))
}

async fn save_target_docker_inventory(
    state: &ApiState,
    target_id: TargetId,
    inventory: PersistedTargetDockerInventory,
) -> Result<(), ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .target_docker_inventory
            .save(target_id, &inventory)
            .await
            .map_err(|_| ApiError::storage())?;
    } else {
        state
            .store
            .write()
            .await
            .target_docker_inventories
            .insert(target_id, inventory);
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
    let resolved = tokio::net::lookup_host((address, AGENT_PORT))
        .await
        .map_err(|_| ApiError::bad_request("invalid_agent_address", "agent address is invalid"))?;
    resolved
        .filter(|address| is_private_network_address(address.ip()))
        .next()
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
        AGENT_PORT, is_private_network_address, load_target_docker_inventory,
        resolve_private_agent_address, save_target_docker_inventory,
    };
    use crate::ApiState;
    use chrono::Utc;
    use lxcup_agent::DockerContainerInfo;
    use lxcup_core::TargetId;
    use lxcup_persistence::PersistedTargetDockerInventory;
    use std::net::IpAddr;

    #[tokio::test]
    async fn in_memory_target_docker_inventory_is_available_after_discovery() {
        let state = ApiState::new();
        let target_id = TargetId::from_uuid(uuid::Uuid::new_v4());
        let inventory = PersistedTargetDockerInventory {
            collected_at: Utc::now(),
            containers: vec![DockerContainerInfo {
                id: "container-1".to_owned(),
                name: "web".to_owned(),
                image: "nginx:latest".to_owned(),
                state: "running".to_owned(),
                status: "Up".to_owned(),
                ports: vec!["80/tcp".to_owned()],
                started_at: None,
                labels: Vec::new(),
            }],
        };

        save_target_docker_inventory(&state, target_id, inventory.clone())
            .await
            .unwrap();

        let loaded = load_target_docker_inventory(&state, target_id)
            .await
            .unwrap()
            .expect("a successful discovery should remain available in memory");
        assert_eq!(loaded.containers, inventory.containers);
        assert_eq!(loaded.collected_at, inventory.collected_at);
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
