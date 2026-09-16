//! Discovery orchestration for Proxmox LXC containers.

use std::{collections::HashSet, sync::Arc};

use chrono::{DateTime, Utc};
use lxcup_core::{
    Container, ContainerId, ContainerManagementState, ContainerStatus, DomainError, Node, NodeId,
    OperatingSystem,
};
use lxcup_persistence::{Repositories, RepositoryError};
use lxcup_proxmox::{
    ProxmoxClient, ProxmoxClientError,
    models::{LxcConfig, LxcContainerSummary},
};
use thiserror::Error;

/// Maps Proxmox inventory and configuration data to a domain container.
pub fn map_lxc_container(
    node_id: NodeId,
    summary: &LxcContainerSummary,
    config: &LxcConfig,
    discovered_at: DateTime<Utc>,
) -> Result<Container, DiscoveryError> {
    let name = config
        .hostname
        .as_deref()
        .or(summary.name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("vmid-{}", summary.vmid));

    let container = Container::new(
        ContainerId::new(summary.vmid),
        node_id,
        name,
        operating_system(config.ostype.as_deref()),
        container_status(&summary.status),
    )?;

    Ok(Container {
        discovered_at,
        ..container
    })
}

fn operating_system(ostype: Option<&str>) -> OperatingSystem {
    match ostype.map(str::to_ascii_lowercase).as_deref() {
        Some(value) if value.contains("debian") => OperatingSystem::Debian,
        Some(value) if value.contains("ubuntu") => OperatingSystem::Ubuntu,
        Some(value) => OperatingSystem::Unknown(value.to_owned()),
        None => OperatingSystem::Unknown("unknown".to_owned()),
    }
}

fn container_status(status: &str) -> ContainerStatus {
    match status {
        "running" => ContainerStatus::Running,
        "stopped" => ContainerStatus::Stopped,
        _ => ContainerStatus::Unknown,
    }
}

/// Result counters and non-fatal item errors from one discovery run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryRunSummary {
    pub inserted: usize,
    pub updated: usize,
    pub missing: usize,
    pub failed: usize,
    pub errors: Vec<DiscoveryItemError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryItemError {
    pub vmid: u64,
    pub message: String,
}

/// Coordinates Proxmox discovery, lifecycle actions and persistence.
pub struct DiscoveryService {
    client: Arc<ProxmoxClient>,
    repositories: Repositories,
}

impl DiscoveryService {
    pub fn new(client: Arc<ProxmoxClient>, repositories: Repositories) -> Self {
        Self {
            client,
            repositories,
        }
    }

    /// Reconciles one node without mutating any target container.
    pub async fn synchronize_node(
        &self,
        node: &Node,
    ) -> Result<DiscoveryRunSummary, DiscoveryError> {
        let summaries = self.client.list_lxc(&node.name).await?;
        let existing = self.repositories.containers.list_by_node(node.id).await?;
        let inventory_ids: HashSet<u64> = summaries.iter().map(|item| item.vmid).collect();
        let mut result = DiscoveryRunSummary::default();

        for summary in summaries {
            let config = match self.client.get_lxc_config(&node.name, summary.vmid).await {
                Ok(config) => config,
                Err(error) => {
                    result.failed += 1;
                    result.errors.push(DiscoveryItemError {
                        vmid: summary.vmid,
                        message: error.to_string(),
                    });
                    continue;
                }
            };

            let container = map_lxc_container(node.id, &summary, &config, Utc::now())?;
            if self
                .repositories
                .containers
                .upsert_discovered(&container)
                .await?
            {
                result.inserted += 1;
            } else {
                result.updated += 1;
            }
        }

        for container in existing {
            if !inventory_ids.contains(&container.id.value()) {
                self.repositories
                    .containers
                    .mark_status_unknown(container.id)
                    .await?;
                result.missing += 1;
            }
        }

        Ok(result)
    }

    /// Applies one explicit management-state transition and persists it.
    pub async fn transition_management_state(
        &self,
        container_id: ContainerId,
        next: ContainerManagementState,
    ) -> Result<Container, DiscoveryError> {
        let mut container = self
            .repositories
            .containers
            .find_by_id(container_id)
            .await?
            .ok_or(DiscoveryError::ContainerNotFound(container_id.value()))?;
        container.transition_management_to(next)?;
        self.repositories
            .containers
            .update_management_state(&container)
            .await?;
        Ok(container)
    }
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("Proxmox discovery failed: {0}")]
    Proxmox(#[source] ProxmoxClientError),
    #[error("discovery persistence failed")]
    Persistence(#[source] RepositoryError),
    #[error("discovery domain mapping failed")]
    Domain(#[source] DomainError),
    #[error("container {0} was not found")]
    ContainerNotFound(u64),
}

impl From<ProxmoxClientError> for DiscoveryError {
    fn from(error: ProxmoxClientError) -> Self {
        Self::Proxmox(error)
    }
}

impl From<RepositoryError> for DiscoveryError {
    fn from(error: RepositoryError) -> Self {
        Self::Persistence(error)
    }
}

impl From<DomainError> for DiscoveryError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> LxcContainerSummary {
        LxcContainerSummary {
            vmid: 101,
            status: "running".to_owned(),
            name: Some("grafana".to_owned()),
            template: Some(0),
            maxmem: None,
            maxdisk: None,
            disk: None,
            uptime: Some(42),
        }
    }

    #[test]
    fn mapping_uses_typed_os_status_and_hostname() {
        let config = LxcConfig {
            hostname: Some("grafana-prod".to_owned()),
            ostype: Some("ubuntu".to_owned()),
            cores: None,
            memory: None,
            swap: None,
            unprivileged: None,
            rootfs: None,
            tags: None,
            extra: Default::default(),
        };

        let container = map_lxc_container(NodeId::new(), &summary(), &config, Utc::now()).unwrap();

        assert_eq!(container.name, "grafana-prod");
        assert_eq!(container.operating_system, OperatingSystem::Ubuntu);
        assert_eq!(container.status, ContainerStatus::Running);
        assert_eq!(
            container.management_state,
            ContainerManagementState::Discovered
        );
    }

    #[test]
    fn mapping_handles_missing_optional_identity_safely() {
        let mut summary = summary();
        summary.name = None;
        summary.status = "paused".to_owned();
        let config = LxcConfig {
            hostname: None,
            ostype: None,
            cores: None,
            memory: None,
            swap: None,
            unprivileged: None,
            rootfs: None,
            tags: None,
            extra: Default::default(),
        };

        let container = map_lxc_container(NodeId::new(), &summary, &config, Utc::now()).unwrap();

        assert_eq!(container.name, "vmid-101");
        assert_eq!(
            container.operating_system,
            OperatingSystem::Unknown("unknown".to_owned())
        );
        assert_eq!(container.status, ContainerStatus::Unknown);
    }
}
