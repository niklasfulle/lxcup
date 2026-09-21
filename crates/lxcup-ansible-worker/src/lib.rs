//! Verträge für den isolierten Ansible-Worker und sein dynamisches Inventory.
//!
//! Das Crate startet selbst keinen Prozess. Es validiert die Übergabe an den
//! späteren Worker, begrenzt Ressourcen und beschreibt Ergebnisse als Events.

use std::collections::{HashMap, HashSet};

use lxcup_ansible::{AnsibleJob, AnsibleJobStatus, PlaybookRegistry};
use lxcup_core::{Container, Node, ResourceTarget, SecretId, Target, TargetTransport};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Secret references required by the worker runtime. Values are resolved only
/// inside the worker and are never part of an API job or inventory payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerSecretRefs {
    pub proxmox_api: SecretId,
    pub proxmox_ca: Option<SecretId>,
    pub ssh_private_key: Option<SecretId>,
    pub ssh_known_hosts: Option<SecretId>,
    pub winrm_username: Option<SecretId>,
    pub winrm_password: Option<SecretId>,
}

impl WorkerSecretRefs {
    fn validate(&self) -> Result<(), WorkerConfigError> {
        let refs = [
            Some(self.proxmox_api),
            self.proxmox_ca,
            self.ssh_private_key,
            self.ssh_known_hosts,
            self.winrm_username,
            self.winrm_password,
        ];
        let mut unique = HashSet::new();
        if refs
            .into_iter()
            .flatten()
            .any(|secret| !unique.insert(secret))
        {
            return Err(WorkerConfigError::DuplicateSecretReference);
        }
        Ok(())
    }
}

/// Worker-only connection configuration. It carries references, not secret
/// material, and can therefore be safely persisted or logged after redaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerConnectionConfig {
    pub proxmox_endpoint: String,
    pub secret_refs: WorkerSecretRefs,
}

impl WorkerConnectionConfig {
    pub fn new(
        proxmox_endpoint: impl Into<String>,
        secret_refs: WorkerSecretRefs,
    ) -> Result<Self, WorkerConfigError> {
        let proxmox_endpoint = proxmox_endpoint.into().trim_end_matches('/').to_owned();
        let host = proxmox_endpoint
            .strip_prefix("https://")
            .unwrap_or_default();
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            return Err(WorkerConfigError::InvalidProxmoxEndpoint);
        }
        secret_refs.validate()?;
        Ok(Self {
            proxmox_endpoint,
            secret_refs,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerSecretPurpose {
    ProxmoxApi,
    ProxmoxCa,
    SshPrivateKey,
    SshKnownHosts,
    WinrmUsername,
    WinrmPassword,
}

/// Boundary used by the worker to obtain credentials from the configured
/// secret provider. Implementations must never pass values to API responses.
pub trait WorkerSecretResolver {
    type Error;

    fn resolve(
        &self,
        secret: SecretId,
        purpose: WorkerSecretPurpose,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// A target generated from persisted lxcup resources, never from a frontend
/// hostname or arbitrary inventory string.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryHost {
    pub key: String,
    pub target: ResourceTarget,
    pub node_id: lxcup_core::NodeId,
    pub address: String,
    pub name: String,
    pub transport: Option<TargetTransport>,
    pub secret_refs: Vec<SecretId>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DynamicInventory {
    pub hosts: Vec<InventoryHost>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WorkerConfigError {
    #[error("the Proxmox worker endpoint must use HTTPS")]
    InvalidProxmoxEndpoint,
    #[error("worker configuration contains duplicate secret references")]
    DuplicateSecretReference,
}

impl DynamicInventory {
    /// Creates the worker inventory from a controller-owned target. No caller
    /// can inject arbitrary hosts or choose a transport outside the target.
    pub fn for_target(target: &Target) -> Result<Self, WorkerContractError> {
        if target.address.trim().is_empty() {
            return Err(WorkerContractError::InvalidInventory);
        }
        Ok(Self {
            hosts: vec![InventoryHost {
                key: format!("target-{}", target.id.as_uuid()),
                target: ResourceTarget::Target(target.id),
                node_id: lxcup_core::NodeId::new(),
                address: target.address.clone(),
                name: target.name.clone(),
                transport: Some(target.transport),
                secret_refs: vec![target.credential_secret_ref],
            }],
        })
    }

    pub fn for_container(
        node: &Node,
        container: &Container,
        secret_refs: Vec<SecretId>,
    ) -> Result<Self, WorkerContractError> {
        if node.id != container.node_id || node.address.trim().is_empty() {
            return Err(WorkerContractError::InvalidInventory);
        }
        if secret_refs.iter().collect::<HashSet<_>>().len() != secret_refs.len() {
            return Err(WorkerContractError::InvalidInventory);
        }
        Ok(Self {
            hosts: vec![InventoryHost {
                key: format!("container-{}", container.id.value()),
                target: ResourceTarget::Container(container.id),
                node_id: node.id,
                address: node.address.clone(),
                name: container.name.clone(),
                transport: None,
                secret_refs,
            }],
        })
    }

    fn host_for(&self, target: ResourceTarget) -> Option<&InventoryHost> {
        self.hosts.iter().find(|host| host.target == target)
    }
}

/// Hard limits applied before a worker process is started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerLimits {
    pub max_parallel_jobs: u16,
    pub max_runtime_seconds: u64,
    pub max_output_bytes: u64,
}

impl WorkerLimits {
    pub fn new(
        max_parallel_jobs: u16,
        max_runtime_seconds: u64,
        max_output_bytes: u64,
    ) -> Result<Self, WorkerContractError> {
        if !(1..=64).contains(&max_parallel_jobs)
            || !(1..=86_400).contains(&max_runtime_seconds)
            || !(1_024..=16_777_216).contains(&max_output_bytes)
        {
            return Err(WorkerContractError::InvalidLimits);
        }
        Ok(Self {
            max_parallel_jobs,
            max_runtime_seconds,
            max_output_bytes,
        })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WorkerContractError {
    #[error("dynamic inventory is invalid")]
    InvalidInventory,
    #[error("worker limits are invalid")]
    InvalidLimits,
    #[error("worker job is not valid for inventory")]
    JobTargetMismatch,
    #[error("worker job is not queued")]
    InvalidJobState,
    #[error("worker job contains an unknown playbook")]
    UnknownPlaybook,
    #[error("worker job references an unavailable secret")]
    MissingSecretReference,
}

/// Validated request handed from backend to the isolated worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWorkerJob {
    pub job: AnsibleJob,
    pub inventory: DynamicInventory,
    pub limits: WorkerLimits,
}

impl ValidatedWorkerJob {
    pub fn accept(
        job: AnsibleJob,
        inventory: DynamicInventory,
        limits: WorkerLimits,
    ) -> Result<Self, WorkerContractError> {
        if job.status != AnsibleJobStatus::Queued {
            return Err(WorkerContractError::InvalidJobState);
        }
        let spec = PlaybookRegistry.resolve(job.operation);
        if job.playbook != spec.playbook || job.playbook_version != spec.version {
            return Err(WorkerContractError::UnknownPlaybook);
        }
        let host = inventory
            .host_for(job.target)
            .ok_or(WorkerContractError::JobTargetMismatch)?;
        if !job
            .secret_refs
            .iter()
            .all(|secret| host.secret_refs.contains(secret))
        {
            return Err(WorkerContractError::MissingSecretReference);
        }
        Ok(Self {
            job,
            inventory,
            limits,
        })
    }
}

/// Structured events deliberately do not carry arbitrary command output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkerEvent {
    Started { job_id: lxcup_core::AnsibleJobId },
    TaskStarted { task: String },
    TaskFinished { task: String, changed: bool },
    Warning { code: WorkerWarning },
    Finished { succeeded: bool },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerWarning {
    Timeout,
    OutputLimitReached,
    ReconciliationRequired,
}

/// Recovery rule used after a worker restart or lost connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    Resume,
    Reconcile,
    Ignore,
}

pub const fn recovery_action(status: AnsibleJobStatus) -> RecoveryAction {
    match status {
        AnsibleJobStatus::Queued | AnsibleJobStatus::Checking | AnsibleJobStatus::Planned => {
            RecoveryAction::Resume
        }
        AnsibleJobStatus::Applying => RecoveryAction::Reconcile,
        AnsibleJobStatus::ReconcileRequired => RecoveryAction::Reconcile,
        AnsibleJobStatus::Succeeded | AnsibleJobStatus::Failed | AnsibleJobStatus::Aborted => {
            RecoveryAction::Ignore
        }
    }
}

/// Converts persisted resources into a stable inventory map for diagnostics.
pub fn inventory_summary(inventory: &DynamicInventory) -> HashMap<String, ResourceTarget> {
    inventory
        .hosts
        .iter()
        .map(|host| (host.key.clone(), host.target))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_ansible::{AnsibleJobRequest, AnsibleOperation, AnsibleParameters, ExecutionMode};
    use lxcup_core::{
        ActorRole, ContainerId, ContainerStatus, OperatingSystem, ResourceLifecycle, SecretId,
    };

    fn resources() -> (Node, Container) {
        let node = Node::new("pve01", "https://pve01:8006").unwrap();
        let container = Container::new(
            ContainerId::new(101),
            node.id,
            "test-lxc",
            OperatingSystem::Debian,
            ContainerStatus::Running,
        )
        .unwrap();
        (node, container)
    }

    #[test]
    fn worker_connection_config_keeps_credentials_as_references() {
        let api = SecretId::new();
        let config = WorkerConnectionConfig::new(
            "https://pve.example.test/",
            WorkerSecretRefs {
                proxmox_api: api,
                proxmox_ca: Some(SecretId::new()),
                ssh_private_key: Some(SecretId::new()),
                ssh_known_hosts: Some(SecretId::new()),
                winrm_username: None,
                winrm_password: None,
            },
        )
        .unwrap();

        assert_eq!(config.proxmox_endpoint, "https://pve.example.test");
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains(&api.as_uuid().to_string()));
        assert!(!serialized.contains("top-secret"));
        assert!(
            WorkerConnectionConfig::new("http://pve.example.test", config.secret_refs.clone())
                .is_err()
        );
    }

    #[test]
    fn worker_connection_config_rejects_duplicate_secret_references() {
        let secret = SecretId::new();
        assert_eq!(
            WorkerConnectionConfig::new(
                "https://pve.example.test",
                WorkerSecretRefs {
                    proxmox_api: secret,
                    proxmox_ca: Some(secret),
                    ssh_private_key: None,
                    ssh_known_hosts: None,
                    winrm_username: None,
                    winrm_password: None,
                },
            ),
            Err(WorkerConfigError::DuplicateSecretReference)
        );
    }

    fn job_request(secret: SecretId) -> AnsibleJobRequest {
        AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: ResourceTarget::Container(ContainerId::new(101)),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
            secret_refs: vec![secret],
            idempotency_key: "worker-1".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        }
    }

    #[test]
    fn inventory_is_derived_from_resources_and_contains_no_free_target() {
        let (node, container) = resources();
        let secret = SecretId::new();
        let inventory = DynamicInventory::for_container(&node, &container, vec![secret]).unwrap();

        assert_eq!(inventory.hosts[0].key, "container-101");
        assert_eq!(
            inventory.hosts[0].target,
            ResourceTarget::Container(container.id)
        );
        assert_eq!(inventory.hosts[0].secret_refs, vec![secret]);
        assert!(DynamicInventory::for_container(&node, &container, vec![secret, secret]).is_err());
    }

    #[test]
    fn target_inventory_uses_the_controller_owned_address_and_transport() {
        let target = lxcup_core::Target::new(
            "windows-01",
            lxcup_core::TargetKind::WindowsServer,
            "windows-01.example.test",
            lxcup_core::TargetTransport::Winrm,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();

        let inventory = DynamicInventory::for_target(&target).unwrap();

        assert_eq!(inventory.hosts[0].target, ResourceTarget::Target(target.id));
        assert_eq!(inventory.hosts[0].transport, Some(TargetTransport::Winrm));
        assert_eq!(inventory.hosts[0].address, "windows-01.example.test");
    }

    #[test]
    fn worker_accepts_only_matching_queued_jobs_and_secret_references() {
        let (node, container) = resources();
        let secret = SecretId::new();
        let job = AnsibleJob::from_request(job_request(secret)).unwrap();
        let inventory = DynamicInventory::for_container(&node, &container, vec![secret]).unwrap();
        let limits = WorkerLimits::new(2, 600, 1_048_576).unwrap();

        assert!(ValidatedWorkerJob::accept(job, inventory, limits).is_ok());
        assert_eq!(
            WorkerLimits::new(0, 600, 1_048_576),
            Err(WorkerContractError::InvalidLimits)
        );
    }

    #[test]
    fn worker_requires_reconciliation_after_apply_or_restart() {
        assert_eq!(
            recovery_action(AnsibleJobStatus::Applying),
            RecoveryAction::Reconcile
        );
        assert_eq!(
            recovery_action(AnsibleJobStatus::Queued),
            RecoveryAction::Resume
        );
        assert_eq!(
            recovery_action(AnsibleJobStatus::Succeeded),
            RecoveryAction::Ignore
        );
    }

    #[test]
    fn events_are_structured_without_terminal_output() {
        let event = WorkerEvent::TaskFinished {
            task: "deploy_agent".to_owned(),
            changed: true,
        };
        let json = serde_json::to_string(&event).unwrap();

        assert!(json.contains("task_finished"));
        assert!(!json.contains("stdout"));
        assert!(!json.contains("stderr"));
    }
}
