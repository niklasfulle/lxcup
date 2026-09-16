use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::{DomainError, DomainResult},
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

impl ContainerManagementState {
    /// Prüft, ob ein Verwaltungszustand direkt erreicht werden darf.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Discovered,
                Self::Managed | Self::Ignored | Self::Disabled
            ) | (Self::Managed, Self::Ignored | Self::Disabled)
                | (Self::Ignored, Self::Managed | Self::Disabled)
                | (Self::Disabled, Self::Managed)
        )
    }
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

    /// Ändert den Verwaltungszustand nach den fachlichen Übergangsregeln.
    pub fn transition_management_to(&mut self, next: ContainerManagementState) -> DomainResult<()> {
        if !self.management_state.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition(
                "container management state",
            ));
        }

        self.management_state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Container, ContainerManagementState, ContainerStatus, OperatingSystem};
    use crate::{ContainerId, NodeId};

    fn container() -> Container {
        Container::new(
            ContainerId::new(101),
            NodeId::new(),
            "grafana",
            OperatingSystem::Debian,
            ContainerStatus::Running,
        )
        .unwrap()
    }

    #[test]
    fn management_can_be_enabled_and_disabled() {
        let mut container = container();

        container
            .transition_management_to(ContainerManagementState::Managed)
            .unwrap();
        container
            .transition_management_to(ContainerManagementState::Disabled)
            .unwrap();

        assert_eq!(
            container.management_state,
            ContainerManagementState::Disabled
        );
    }

    #[test]
    fn cannot_reactivate_disabled_container_as_ignored() {
        let mut container = container();
        container
            .transition_management_to(ContainerManagementState::Disabled)
            .unwrap();

        assert!(
            container
                .transition_management_to(ContainerManagementState::Ignored)
                .is_err()
        );
    }
}
