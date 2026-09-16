use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::DomainError,
    ids::{ContainerId, NodeId},
};

/// Laufzeitstatus eines LXC-Containers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContainerStatus {
    Running,
    Stopped,
    Unknown,
}

/// Verwaltungszustand eines entdeckten Containers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContainerManagementState {
    Discovered,
    Managed,
    Ignored,
    Disabled,
}

/// Unterstützte oder unbekannte Betriebssystemfamilie eines Containers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperatingSystem {
    Debian,
    Ubuntu,
    Unknown(String),
}

/// Ein von lxcup entdeckter Proxmox-LXC-Container.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Container {
    pub id: ContainerId,
    pub node_id: NodeId,
    pub name: String,
    pub operating_system: OperatingSystem,
    pub status: ContainerStatus,
    pub management_state: ContainerManagementState,
    pub discovered_at: DateTime<Utc>,
}

impl Container {
    pub fn new(
        id: ContainerId,
        node_id: NodeId,
        name: impl Into<String>,
        operating_system: OperatingSystem,
        status: ContainerStatus,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "container name",
            });
        }

        Ok(Self {
            id,
            node_id,
            name,
            operating_system,
            status,
            management_state: ContainerManagementState::Discovered,
            discovered_at: Utc::now(),
        })
    }
}
