use serde::{Deserialize, Serialize};

use crate::{ContainerId, DomainError, NodeId, TargetId, error::DomainResult};

/// Stable target type used by API DTOs, audit events and Ansible jobs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceTarget {
    Target(TargetId),
    Node(NodeId),
    Container(ContainerId),
}

/// Lifecycle shared by resources that can be managed by lxcup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLifecycle {
    Pending,
    Discovered,
    Managed,
    Failed,
    Disabled,
}

impl ResourceLifecycle {
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Pending | Self::Discovered | Self::Failed
            ) | (
                Self::Discovered,
                Self::Discovered | Self::Managed | Self::Disabled | Self::Failed
            ) | (
                Self::Managed,
                Self::Managed | Self::Pending | Self::Disabled | Self::Failed
            ) | (Self::Failed, Self::Failed | Self::Pending | Self::Disabled)
                | (Self::Disabled, Self::Disabled | Self::Pending)
        )
    }
}

/// Coarse-grained permissions used at the API boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Read,
    Configure,
    DeployAgent,
    Update,
    Destructive,
}

/// Roles are intentionally ordered by capability, not by UI labels.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    Viewer,
    Operator,
    Admin,
}

impl ActorRole {
    pub const fn grants(self, permission: Permission) -> bool {
        match self {
            Self::Viewer => matches!(permission, Permission::Read),
            Self::Operator => matches!(
                permission,
                Permission::Read
                    | Permission::Configure
                    | Permission::DeployAgent
                    | Permission::Update
            ),
            Self::Admin => true,
        }
    }
}

/// Operations exposed by lxcup. There is no free-form shell/playbook action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAction {
    Discover,
    HealthCheck,
    ConfigureTarget,
    DeployAgent,
    UpdateAgent,
    RepairAgent,
    UpdatePackages,
    DisableResource,
    DeleteResource,
}

impl ResourceAction {
    pub const fn permission(self) -> Permission {
        match self {
            Self::Discover | Self::HealthCheck => Permission::Read,
            Self::ConfigureTarget => Permission::Configure,
            Self::DeployAgent => Permission::DeployAgent,
            Self::UpdateAgent | Self::RepairAgent | Self::UpdatePackages => Permission::Update,
            Self::DisableResource | Self::DeleteResource => Permission::Destructive,
        }
    }

    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::DisableResource | Self::DeleteResource)
    }

    pub const fn supports(self, target: ResourceTarget) -> bool {
        match target {
            ResourceTarget::Target(_) => true,
            ResourceTarget::Node(_) => matches!(
                self,
                Self::Discover
                    | Self::HealthCheck
                    | Self::ConfigureTarget
                    | Self::DisableResource
                    | Self::DeleteResource
            ),
            ResourceTarget::Container(_) => true,
        }
    }
}

/// Validates the server-side safety rules before any worker or adapter call.
pub fn validate_action(
    role: ActorRole,
    target: ResourceTarget,
    lifecycle: ResourceLifecycle,
    action: ResourceAction,
    confirmed: bool,
    idempotency_key: &str,
) -> DomainResult<()> {
    if !action.supports(target) {
        return Err(DomainError::InvalidStateTransition(
            "action is not supported for resource target",
        ));
    }
    if !role.grants(action.permission()) {
        return Err(DomainError::PermissionDenied);
    }
    if lifecycle == ResourceLifecycle::Disabled
        && !matches!(
            action,
            ResourceAction::Discover | ResourceAction::HealthCheck
        )
    {
        return Err(DomainError::InvalidStateTransition(
            "disabled resource cannot run this action",
        ));
    }
    if action.is_destructive() && !confirmed {
        return Err(DomainError::ConfirmationRequired);
    }
    if idempotency_key.trim().is_empty() {
        return Err(DomainError::EmptyValue {
            field: "idempotency key",
        });
    }
    if idempotency_key.chars().count() > 200 {
        return Err(DomainError::InvalidStateTransition(
            "idempotency key is too long",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ActorRole, Permission, ResourceAction, ResourceLifecycle, ResourceTarget, validate_action,
    };
    use crate::{ContainerId, NodeId};

    #[test]
    fn lifecycle_requires_explicit_safe_transitions() {
        assert!(ResourceLifecycle::Pending.can_transition_to(ResourceLifecycle::Discovered));
        assert!(ResourceLifecycle::Discovered.can_transition_to(ResourceLifecycle::Managed));
        assert!(!ResourceLifecycle::Pending.can_transition_to(ResourceLifecycle::Managed));
        assert!(!ResourceLifecycle::Disabled.can_transition_to(ResourceLifecycle::Managed));
    }

    #[test]
    fn roles_and_targets_are_enforced_before_execution() {
        let container = ResourceTarget::Container(ContainerId::new(101));
        assert!(
            validate_action(
                ActorRole::Operator,
                container,
                ResourceLifecycle::Managed,
                ResourceAction::DeployAgent,
                false,
                "deploy-1"
            )
            .is_ok()
        );
        assert_eq!(
            validate_action(
                ActorRole::Viewer,
                container,
                ResourceLifecycle::Managed,
                ResourceAction::DeployAgent,
                false,
                "deploy-2"
            ),
            Err(crate::DomainError::PermissionDenied)
        );
        assert!(
            validate_action(
                ActorRole::Operator,
                ResourceTarget::Node(NodeId::new()),
                ResourceLifecycle::Managed,
                ResourceAction::DeployAgent,
                false,
                "deploy-3"
            )
            .is_err()
        );
    }

    #[test]
    fn destructive_actions_need_admin_and_confirmation() {
        let target = ResourceTarget::Container(ContainerId::new(101));
        assert_eq!(
            validate_action(
                ActorRole::Operator,
                target,
                ResourceLifecycle::Managed,
                ResourceAction::DeleteResource,
                true,
                "delete-1"
            ),
            Err(crate::DomainError::PermissionDenied)
        );
        assert_eq!(
            validate_action(
                ActorRole::Admin,
                target,
                ResourceLifecycle::Managed,
                ResourceAction::DeleteResource,
                false,
                "delete-2"
            ),
            Err(crate::DomainError::ConfirmationRequired)
        );
        assert!(
            validate_action(
                ActorRole::Admin,
                target,
                ResourceLifecycle::Managed,
                ResourceAction::DeleteResource,
                true,
                "delete-3"
            )
            .is_ok()
        );
    }

    #[test]
    fn permissions_are_explicitly_ordered() {
        assert!(ActorRole::Viewer.grants(Permission::Read));
        assert!(!ActorRole::Viewer.grants(Permission::Configure));
        assert!(ActorRole::Admin.grants(Permission::Destructive));
    }
}
