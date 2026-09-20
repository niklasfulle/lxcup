//! Validierter Ausführungsvertrag für den isolierten Ansible-Worker.
//!
//! Requests enthalten nur Registry-Operationen, typisierte Parameter und
//! Secret-Referenzen. Freie Playbook-Pfade, Shell-Kommandos und Secret-Werte
//! sind absichtlich nicht Teil dieses öffentlichen Modells.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use lxcup_core::{
    ActorRole, AnsibleJobId, ResourceAction, ResourceLifecycle, ResourceTarget, SecretId,
    validate_action,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Allowlisted operations exposed to the API and worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsibleOperation {
    DeployAgent,
    UpdateAgent,
    UpdatePackages,
    RepairAgent,
    ConfigureTarget,
    HealthCheck,
}

/// Execution phase requested by the caller.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Check,
    Plan,
    Apply,
    Reconcile,
}

/// Lifecycle of a validated Ansible task.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsibleJobStatus {
    Queued,
    Checking,
    Planned,
    Applying,
    ReconcileRequired,
    Succeeded,
    Failed,
    Aborted,
}

impl AnsibleJobStatus {
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Checking | Self::Planned | Self::Aborted)
                | (Self::Checking, Self::Planned | Self::Failed | Self::Aborted)
                | (Self::Planned, Self::Applying | Self::Aborted)
                | (
                    Self::Applying,
                    Self::Succeeded | Self::Failed | Self::ReconcileRequired
                )
                | (Self::ReconcileRequired, Self::Succeeded | Self::Failed)
        )
    }
}

/// Typed variables for each allowlisted operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AnsibleParameters {
    DeployAgent { agent_version: String },
    UpdateAgent { agent_version: String },
    UpdatePackages { packages: Vec<String> },
    RepairAgent,
    ConfigureTarget { desired_hostname: Option<String> },
    HealthCheck,
}

impl AnsibleParameters {
    fn operation(&self) -> AnsibleOperation {
        match self {
            Self::DeployAgent { .. } => AnsibleOperation::DeployAgent,
            Self::UpdateAgent { .. } => AnsibleOperation::UpdateAgent,
            Self::UpdatePackages { .. } => AnsibleOperation::UpdatePackages,
            Self::RepairAgent => AnsibleOperation::RepairAgent,
            Self::ConfigureTarget { .. } => AnsibleOperation::ConfigureTarget,
            Self::HealthCheck => AnsibleOperation::HealthCheck,
        }
    }

    fn validate(&self) -> Result<(), AnsibleContractError> {
        match self {
            Self::DeployAgent { agent_version } | Self::UpdateAgent { agent_version }
                if agent_version.trim().is_empty() || agent_version.len() > 50 =>
            {
                Err(AnsibleContractError::Invalid)
            }
            Self::UpdatePackages { packages } if packages.is_empty() || packages.len() > 100 => {
                Err(AnsibleContractError::Invalid)
            }
            Self::UpdatePackages { packages }
                if packages
                    .iter()
                    .any(|package| package.trim().is_empty() || package.chars().count() > 100) =>
            {
                Err(AnsibleContractError::Invalid)
            }
            Self::ConfigureTarget {
                desired_hostname: Some(hostname),
            } if hostname.trim().is_empty() || hostname.chars().count() > 253 => {
                Err(AnsibleContractError::Invalid)
            }
            _ => Ok(()),
        }
    }
}

/// Versioned allowlist. The worker receives only the resolved specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybookSpec {
    pub operation: AnsibleOperation,
    pub playbook: &'static str,
    pub version: &'static str,
    pub action: ResourceAction,
    pub requires_secret: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlaybookRegistry;

impl PlaybookRegistry {
    pub const fn resolve(self, operation: AnsibleOperation) -> PlaybookSpec {
        match operation {
            AnsibleOperation::DeployAgent => PlaybookSpec {
                operation,
                playbook: "agent/deploy.yml",
                version: "1",
                action: ResourceAction::DeployAgent,
                requires_secret: true,
            },
            AnsibleOperation::UpdateAgent => PlaybookSpec {
                operation,
                playbook: "agent/update.yml",
                version: "1",
                action: ResourceAction::UpdateAgent,
                requires_secret: true,
            },
            AnsibleOperation::UpdatePackages => PlaybookSpec {
                operation,
                playbook: "packages/update.yml",
                version: "1",
                action: ResourceAction::UpdatePackages,
                requires_secret: true,
            },
            AnsibleOperation::RepairAgent => PlaybookSpec {
                operation,
                playbook: "agent/repair.yml",
                version: "1",
                action: ResourceAction::RepairAgent,
                requires_secret: true,
            },
            AnsibleOperation::ConfigureTarget => PlaybookSpec {
                operation,
                playbook: "target/configure.yml",
                version: "1",
                action: ResourceAction::ConfigureTarget,
                requires_secret: true,
            },
            AnsibleOperation::HealthCheck => PlaybookSpec {
                operation,
                playbook: "target/health-check.yml",
                version: "1",
                action: ResourceAction::HealthCheck,
                requires_secret: false,
            },
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AnsibleContractError {
    #[error("ansible request is invalid")]
    Invalid,
    #[error("ansible operation is not allowed")]
    PermissionDenied,
    #[error("ansible operation requires confirmation")]
    ConfirmationRequired,
    #[error("ansible target is not supported")]
    UnsupportedTarget,
    #[error("ansible operation requires a secret reference")]
    MissingSecretReference,
    #[error("ansible job state transition is invalid")]
    InvalidTransition,
}

/// Input accepted by the backend. It has no free-form command or path field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnsibleJobRequest {
    pub operation: AnsibleOperation,
    pub target: ResourceTarget,
    pub lifecycle: ResourceLifecycle,
    pub mode: ExecutionMode,
    pub parameters: AnsibleParameters,
    pub secret_refs: Vec<SecretId>,
    pub idempotency_key: String,
    pub confirmed: bool,
    pub actor_role: ActorRole,
}

/// Persistable, audit-safe representation of an Ansible task.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnsibleJob {
    pub id: AnsibleJobId,
    pub operation: AnsibleOperation,
    pub playbook: String,
    pub playbook_version: String,
    pub target: ResourceTarget,
    pub mode: ExecutionMode,
    pub parameters: AnsibleParameters,
    pub secret_refs: Vec<SecretId>,
    pub parameter_hash: String,
    pub status: AnsibleJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AnsibleJob {
    pub fn from_request(request: AnsibleJobRequest) -> Result<Self, AnsibleContractError> {
        if request.parameters.operation() != request.operation {
            return Err(AnsibleContractError::Invalid);
        }
        request.parameters.validate()?;
        if request.idempotency_key.trim().is_empty() || request.idempotency_key.len() > 200 {
            return Err(AnsibleContractError::Invalid);
        }
        let registry = PlaybookRegistry;
        let spec = registry.resolve(request.operation);
        if !spec.action.supports(request.target) {
            return Err(AnsibleContractError::UnsupportedTarget);
        }
        validate_action(
            request.actor_role,
            request.target,
            request.lifecycle,
            spec.action,
            request.confirmed,
            &request.idempotency_key,
        )
        .map_err(|error| match error {
            lxcup_core::DomainError::PermissionDenied => AnsibleContractError::PermissionDenied,
            lxcup_core::DomainError::ConfirmationRequired => {
                AnsibleContractError::ConfirmationRequired
            }
            lxcup_core::DomainError::InvalidStateTransition(_) => {
                AnsibleContractError::UnsupportedTarget
            }
            _ => AnsibleContractError::Invalid,
        })?;
        if request.mode == ExecutionMode::Apply
            && spec.action.permission() != lxcup_core::Permission::Read
            && !request.confirmed
        {
            return Err(AnsibleContractError::ConfirmationRequired);
        }
        if spec.requires_secret && request.secret_refs.is_empty() {
            return Err(AnsibleContractError::MissingSecretReference);
        }
        let mut unique_refs = HashSet::new();
        if request
            .secret_refs
            .iter()
            .any(|id| !unique_refs.insert(*id))
        {
            return Err(AnsibleContractError::Invalid);
        }
        let parameter_hash = parameter_hash(&request, spec);
        let now = Utc::now();
        Ok(Self {
            id: AnsibleJobId::new(),
            operation: request.operation,
            playbook: spec.playbook.to_owned(),
            playbook_version: spec.version.to_owned(),
            target: request.target,
            mode: request.mode,
            parameters: request.parameters,
            secret_refs: request.secret_refs,
            parameter_hash,
            status: AnsibleJobStatus::Queued,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn transition_to(&mut self, next: AnsibleJobStatus) -> Result<(), AnsibleContractError> {
        if !self.status.can_transition_to(next) {
            return Err(AnsibleContractError::InvalidTransition);
        }
        self.status = next;
        self.updated_at = Utc::now();
        Ok(())
    }
}

fn parameter_hash(request: &AnsibleJobRequest, spec: PlaybookSpec) -> String {
    let canonical = serde_json::json!({
        "operation": request.operation,
        "playbook": spec.playbook,
        "playbook_version": spec.version,
        "target": request.target,
        "mode": request.mode,
        "parameters": request.parameters,
        "secret_refs": request.secret_refs,
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&canonical).expect("contract types are serializable"));
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{ContainerId, SecretId};

    fn request(operation: AnsibleOperation, parameters: AnsibleParameters) -> AnsibleJobRequest {
        AnsibleJobRequest {
            operation,
            target: ResourceTarget::Container(ContainerId::new(101)),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters,
            secret_refs: vec![SecretId::new()],
            idempotency_key: "job-1".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        }
    }

    #[test]
    fn registry_resolves_only_versioned_allowlisted_playbooks() {
        let spec = PlaybookRegistry.resolve(AnsibleOperation::DeployAgent);

        assert_eq!(spec.playbook, "agent/deploy.yml");
        assert_eq!(spec.version, "1");
        assert!(spec.requires_secret);
    }

    #[test]
    fn job_contains_hash_and_never_secret_values() {
        let job = AnsibleJob::from_request(request(
            AnsibleOperation::UpdatePackages,
            AnsibleParameters::UpdatePackages {
                packages: vec!["nginx".to_owned()],
            },
        ))
        .unwrap();
        let json = serde_json::to_string(&job).unwrap();

        assert_eq!(job.playbook, "packages/update.yml");
        assert_eq!(job.playbook_version, "1");
        assert_eq!(job.parameter_hash.len(), 64);
        assert!(json.contains("secret_refs"));
        assert!(!json.contains("top-secret"));
    }

    #[test]
    fn typed_parameters_and_policy_are_enforced() {
        let mut invalid = request(
            AnsibleOperation::UpdateAgent,
            AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
        );
        assert_eq!(
            AnsibleJob::from_request(invalid.clone()),
            Err(AnsibleContractError::Invalid)
        );

        invalid = request(
            AnsibleOperation::DeployAgent,
            AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
        );
        invalid.actor_role = ActorRole::Viewer;
        assert_eq!(
            AnsibleJob::from_request(invalid),
            Err(AnsibleContractError::PermissionDenied)
        );

        let mut missing_secret = request(
            AnsibleOperation::DeployAgent,
            AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
        );
        missing_secret.secret_refs.clear();
        assert_eq!(
            AnsibleJob::from_request(missing_secret),
            Err(AnsibleContractError::MissingSecretReference)
        );
    }

    #[test]
    fn job_transitions_require_worker_reconciliation_after_apply_loss() {
        let mut job = AnsibleJob::from_request(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap();
        job.transition_to(AnsibleJobStatus::Checking).unwrap();
        job.transition_to(AnsibleJobStatus::Planned).unwrap();
        job.transition_to(AnsibleJobStatus::Applying).unwrap();
        job.transition_to(AnsibleJobStatus::ReconcileRequired)
            .unwrap();
        assert!(job.transition_to(AnsibleJobStatus::Succeeded).is_ok());
        assert_eq!(
            job.transition_to(AnsibleJobStatus::Applying),
            Err(AnsibleContractError::InvalidTransition)
        );
    }
}
