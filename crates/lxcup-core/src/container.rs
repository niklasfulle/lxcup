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

/// Sichere, fest definierte Aktionen für den Lebenszyklus eines LXCs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerAction {
    Start,
    Stop,
    Shutdown,
    Reboot,
    Refresh,
    Clone,
    Backup,
    Restore,
    Delete,
}

impl ContainerAction {
    pub const fn requires_confirmation(self) -> bool {
        matches!(
            self,
            Self::Clone | Self::Backup | Self::Restore | Self::Delete
        )
    }

    pub const fn is_supported_for(self, status: ContainerStatus) -> bool {
        match self {
            Self::Start => matches!(status, ContainerStatus::Stopped),
            Self::Stop | Self::Shutdown => matches!(status, ContainerStatus::Running),
            Self::Reboot => matches!(status, ContainerStatus::Running),
            Self::Refresh => true,
            Self::Clone | Self::Backup | Self::Restore | Self::Delete => true,
        }
    }
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

/// Ein historischer LXC-Datensatz; neue Ressourcen werden als Target angelegt.
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

    /// Validiert eine Aktion, bevor ein Adapter oder Worker angesprochen wird.
    pub fn validate_action(
        &self,
        action: ContainerAction,
        confirmed: bool,
        idempotency_key: &str,
    ) -> DomainResult<()> {
        if self.management_state == ContainerManagementState::Disabled
            && !matches!(action, ContainerAction::Refresh)
        {
            return Err(DomainError::InvalidStateTransition(
                "disabled container cannot run lifecycle actions",
            ));
        }
        if !action.is_supported_for(self.status) {
            return Err(DomainError::InvalidStateTransition(
                "container status does not support this action",
            ));
        }
        if action.requires_confirmation() && !confirmed {
            return Err(DomainError::ConfirmationRequired);
        }
        if idempotency_key.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "idempotency key",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Container, ContainerAction, ContainerManagementState, ContainerStatus, OperatingSystem,
    };
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

    #[test]
    fn lifecycle_actions_fail_closed_on_status_and_confirmation() {
        let mut container = container();
        assert!(
            container
                .validate_action(ContainerAction::Start, false, "start-1")
                .is_err()
        );
        assert!(
            container
                .validate_action(ContainerAction::Shutdown, false, "shutdown-1")
                .is_ok()
        );
        assert_eq!(
            container.validate_action(ContainerAction::Delete, false, "delete-1"),
            Err(crate::DomainError::ConfirmationRequired)
        );
        container.management_state = ContainerManagementState::Disabled;
        assert!(
            container
                .validate_action(ContainerAction::Refresh, false, "refresh-1")
                .is_ok()
        );
        assert!(
            container
                .validate_action(ContainerAction::Reboot, true, "reboot-1")
                .is_err()
        );
    }
}
