//! Validierter Ausführungsvertrag für den isolierten Ansible-Worker.
//!
//! Requests enthalten nur Registry-Operationen, typisierte Parameter und
//! Secret-Referenzen. Freie Playbook-Pfade, Shell-Kommandos und Secret-Werte
//! sind absichtlich nicht Teil dieses öffentlichen Modells.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use lxcup_core::{
    ActorRole, AnsibleJobId, ResourceAction, ResourceLifecycle, ResourceTarget, SecretId,
    validate_action,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

const RECONCILE_KEY_PREFIX: &str = "reconcile-job:";

pub fn reconcile_idempotency_key(source_job_id: AnsibleJobId) -> String {
    format!("{RECONCILE_KEY_PREFIX}{}", source_job_id.as_uuid())
}

pub fn reconciliation_source_job_id(idempotency_key: &str) -> Option<AnsibleJobId> {
    Uuid::parse_str(idempotency_key.strip_prefix(RECONCILE_KEY_PREFIX)?)
        .ok()
        .map(AnsibleJobId::from_uuid)
}

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
    CollectPackageInventory,
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

impl AnsibleOperation {
    pub const fn can_reconcile_apply(self) -> bool {
        matches!(
            self,
            Self::DeployAgent | Self::UpdateAgent | Self::RepairAgent | Self::UpdatePackages
        )
    }

    /// Returns the modes registered by the shared frontend/backend contract.
    pub fn supported_modes(self) -> &'static [ExecutionMode] {
        static MODES: OnceLock<HashMap<AnsibleOperation, Vec<ExecutionMode>>> = OnceLock::new();
        MODES
            .get_or_init(|| {
                serde_json::from_str(include_str!("../../../frontend/workflow-modes.json"))
                    .expect("shared workflow mode contract must be valid")
            })
            .get(&self)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn supports_mode(self, mode: ExecutionMode) -> bool {
        self.supported_modes().contains(&mode)
    }
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
                | (
                    Self::Planned,
                    Self::Applying | Self::Succeeded | Self::Failed | Self::Aborted
                )
                | (
                    Self::Applying,
                    Self::Succeeded | Self::Failed | Self::ReconcileRequired
                )
                | (Self::ReconcileRequired, Self::Succeeded | Self::Failed)
                | (Self::Failed, Self::Queued)
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
    CollectPackageInventory,
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
            Self::CollectPackageInventory => AnsibleOperation::CollectPackageInventory,
        }
    }

    fn validate(&self) -> Result<(), AnsibleContractError> {
        match self {
            Self::DeployAgent { agent_version } | Self::UpdateAgent { agent_version }
                if agent_version.trim().is_empty() || agent_version.len() > 50 =>
            {
                Err(AnsibleContractError::Invalid)
            }
            Self::UpdatePackages { packages }
                if packages.is_empty()
                    || packages.len() > 100
                    || (packages.iter().any(|package| package == "*")
                        && packages.as_slice() != ["*"]) =>
            {
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
            AnsibleOperation::CollectPackageInventory => PlaybookSpec {
                operation,
                playbook: "inventory/packages.yml",
                version: "1",
                action: ResourceAction::CollectPackageInventory,
                requires_secret: true,
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
    #[error("ansible execution mode is not supported for this operation")]
    UnsupportedMode,
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
    pub idempotency_key: String,
    pub parameter_hash: String,
    pub status: AnsibleJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AnsibleJob {
    pub fn can_be_reconciled(&self) -> bool {
        self.status == AnsibleJobStatus::ReconcileRequired
            && self.mode == ExecutionMode::Apply
            && self.operation.can_reconcile_apply()
    }

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
        if !request.operation.supports_mode(request.mode) {
            return Err(AnsibleContractError::UnsupportedMode);
        }
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
            idempotency_key: request.idempotency_key,
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

/// Audit-safe events persisted for an Ansible job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum JobEventKind {
    Queued,
    StatusChanged {
        status: AnsibleJobStatus,
    },
    TaskStarted {
        task: String,
    },
    TaskFinished {
        task: String,
        changed: bool,
    },
    Failed {
        code: JobFailureCode,
    },
    ReconcileRequired,
    /// Redacted output emitted by the isolated worker. Secret values are never
    /// included; the UI may safely render this as the job's technical log.
    WorkerLog {
        source: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobFailureCode {
    WorkerUnavailable,
    InvalidCredentials,
    HostKeyChanged,
    Unreachable,
    Timeout,
    PlaybookFailed,
    WorkerRestarted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobEvent {
    pub sequence: u64,
    pub job_id: AnsibleJobId,
    pub event: JobEventKind,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobSubmission {
    Created(AnsibleJob),
    Duplicate(AnsibleJob),
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CoordinatorError {
    #[error("target already has an active ansible job")]
    TargetBusy,
    #[error("ansible job was not found")]
    NotFound,
    #[error("ansible job cannot be retried safely")]
    RetryNotAllowed,
    #[error("ansible job is not eligible for reconciliation")]
    ReconcileNotAllowed,
    #[error("ansible job cannot transition to the requested state")]
    InvalidTransition,
    #[error("ansible job cannot accept another event")]
    Terminal,
    #[error(transparent)]
    Contract(#[from] AnsibleContractError),
}

/// In-process orchestration contract. Persistence adapters can mirror these
/// mutations to PostgreSQL without changing API or worker semantics.
#[derive(Default)]
pub struct AnsibleJobCoordinator {
    jobs: HashMap<AnsibleJobId, AnsibleJob>,
    idempotency: HashMap<(ResourceTarget, String), AnsibleJobId>,
    active_targets: HashMap<ResourceTarget, AnsibleJobId>,
    events: HashMap<AnsibleJobId, Vec<JobEvent>>,
}

impl AnsibleJobCoordinator {
    pub fn target_has_active_jobs(&self, target_id: lxcup_core::TargetId) -> bool {
        let target = ResourceTarget::Target(target_id);
        self.active_targets.contains_key(&target)
            || self.jobs.values().any(|job| {
                job.target == target
                    && matches!(
                        job.status,
                        AnsibleJobStatus::Checking
                            | AnsibleJobStatus::Planned
                            | AnsibleJobStatus::Applying
                            | AnsibleJobStatus::ReconcileRequired
                    )
            })
    }

    /// Removes queued and historical in-memory jobs for one registered target.
    /// Active work must be allowed to finish before its target can be deleted.
    pub fn remove_target_jobs(
        &mut self,
        target_id: lxcup_core::TargetId,
    ) -> Result<(), CoordinatorError> {
        let target = ResourceTarget::Target(target_id);
        if self.target_has_active_jobs(target_id) {
            return Err(CoordinatorError::TargetBusy);
        }
        let removed_ids = self
            .jobs
            .values()
            .filter(|job| job.target == target)
            .map(|job| job.id)
            .collect::<std::collections::HashSet<_>>();
        self.jobs.retain(|id, _| !removed_ids.contains(id));
        self.events.retain(|id, _| !removed_ids.contains(id));
        self.idempotency
            .retain(|(job_target, _), id| *job_target != target && !removed_ids.contains(id));
        Ok(())
    }

    pub fn submit(
        &mut self,
        request: AnsibleJobRequest,
    ) -> Result<JobSubmission, CoordinatorError> {
        let key = (request.target, request.idempotency_key.clone());
        if let Some(existing_id) = self.idempotency.get(&key).copied() {
            let existing = self
                .jobs
                .get(&existing_id)
                .ok_or(CoordinatorError::NotFound)?;
            return Ok(JobSubmission::Duplicate(existing.clone()));
        }
        let job = AnsibleJob::from_request(request)?;
        self.idempotency.insert(key, job.id);
        self.record_internal(job.id, JobEventKind::Queued);
        self.jobs.insert(job.id, job.clone());
        Ok(JobSubmission::Created(job))
    }

    pub fn submit_reconciliation(
        &mut self,
        source: &AnsibleJob,
    ) -> Result<JobSubmission, CoordinatorError> {
        if !source.can_be_reconciled() {
            return Err(CoordinatorError::ReconcileNotAllowed);
        }
        let key = reconcile_idempotency_key(source.id);
        if let Some(existing_id) = self.idempotency.get(&(source.target, key.clone())).copied() {
            let existing = self
                .jobs
                .get(&existing_id)
                .ok_or(CoordinatorError::NotFound)?;
            return Ok(JobSubmission::Duplicate(existing.clone()));
        }
        if self
            .active_targets
            .get(&source.target)
            .is_some_and(|active_job_id| *active_job_id != source.id)
        {
            return Err(CoordinatorError::TargetBusy);
        }
        let now = Utc::now();
        let mut job = source.clone();
        job.id = AnsibleJobId::new();
        job.mode = ExecutionMode::Reconcile;
        job.idempotency_key = key.clone();
        job.status = AnsibleJobStatus::Queued;
        job.created_at = now;
        job.updated_at = now;
        self.active_targets.insert(job.target, job.id);
        self.idempotency.insert((job.target, key), job.id);
        self.record_internal(job.id, JobEventKind::Queued);
        self.jobs.insert(job.id, job.clone());
        Ok(JobSubmission::Created(job))
    }

    pub fn job(&self, id: AnsibleJobId) -> Result<AnsibleJob, CoordinatorError> {
        self.jobs
            .get(&id)
            .cloned()
            .ok_or(CoordinatorError::NotFound)
    }

    pub fn jobs(&self) -> Vec<AnsibleJob> {
        let mut jobs = self.jobs.values().cloned().collect::<Vec<_>>();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs
    }

    pub fn events(&self, id: AnsibleJobId) -> Result<Vec<JobEvent>, CoordinatorError> {
        if !self.jobs.contains_key(&id) {
            return Err(CoordinatorError::NotFound);
        }
        Ok(self.events.get(&id).cloned().unwrap_or_default())
    }

    pub fn transition(
        &mut self,
        id: AnsibleJobId,
        next: AnsibleJobStatus,
    ) -> Result<AnsibleJob, CoordinatorError> {
        let target = self.jobs.get(&id).ok_or(CoordinatorError::NotFound)?.target;
        if next == AnsibleJobStatus::Checking
            && self
                .active_targets
                .get(&target)
                .is_some_and(|active| *active != id)
        {
            return Err(CoordinatorError::TargetBusy);
        }
        let job = self.jobs.get_mut(&id).ok_or(CoordinatorError::NotFound)?;
        job.transition_to(next)
            .map_err(|_| CoordinatorError::InvalidTransition)?;
        let job_snapshot = job.clone();
        if next == AnsibleJobStatus::Checking {
            self.active_targets.insert(target, id);
        }
        if matches!(
            next,
            AnsibleJobStatus::Succeeded | AnsibleJobStatus::Failed | AnsibleJobStatus::Aborted
        ) {
            self.active_targets.remove(&target);
        }
        self.record_internal(id, JobEventKind::StatusChanged { status: next });
        if next == AnsibleJobStatus::ReconcileRequired {
            self.record_internal(id, JobEventKind::ReconcileRequired);
        }
        Ok(job_snapshot)
    }

    pub fn record_event(
        &mut self,
        id: AnsibleJobId,
        event: JobEventKind,
    ) -> Result<(), CoordinatorError> {
        let job = self.jobs.get(&id).ok_or(CoordinatorError::NotFound)?;
        if matches!(
            job.status,
            AnsibleJobStatus::Succeeded | AnsibleJobStatus::Failed | AnsibleJobStatus::Aborted
        ) {
            return Err(CoordinatorError::Terminal);
        }
        self.record_internal(id, event);
        Ok(())
    }

    pub fn retry(&mut self, id: AnsibleJobId) -> Result<AnsibleJob, CoordinatorError> {
        let job = self.jobs.get(&id).ok_or(CoordinatorError::NotFound)?;
        if self
            .active_targets
            .get(&job.target)
            .is_some_and(|active| *active != id)
        {
            return Err(CoordinatorError::TargetBusy);
        }
        if job.status != AnsibleJobStatus::Failed
            || (job.operation == AnsibleOperation::UpdatePackages
                && job.mode == ExecutionMode::Apply)
        {
            return Err(CoordinatorError::RetryNotAllowed);
        }
        let job_snapshot = {
            let job = self.jobs.get_mut(&id).ok_or(CoordinatorError::NotFound)?;
            job.transition_to(AnsibleJobStatus::Queued)
                .map_err(|_| CoordinatorError::InvalidTransition)?;
            job.clone()
        };
        self.record_internal(
            id,
            JobEventKind::StatusChanged {
                status: AnsibleJobStatus::Queued,
            },
        );
        Ok(job_snapshot)
    }

    fn record_internal(&mut self, id: AnsibleJobId, event: JobEventKind) {
        let events = self.events.entry(id).or_default();
        events.push(JobEvent {
            sequence: events.len() as u64 + 1,
            job_id: id,
            event,
            created_at: Utc::now(),
        });
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
#[path = "lib_tests.rs"]
mod tests;
