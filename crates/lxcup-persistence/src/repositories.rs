//! SQL-Repositorys für die MVP-Domain.

use chrono::{DateTime, Utc};
use lxcup_ansible::{AnsibleJob, AnsibleJobStatus, JobEvent};
use lxcup_core::{
    AgentRegistration, AvailableUpdate, Container, ContainerId, ContainerManagementState,
    ContainerStatus, DockerWorkload, DockerWorkloadManagementState, EnvironmentStatus, Execution,
    ExecutionId, ExecutionStatus, Node, NodeId, NodeStatus, OperatingSystem, PackageChangeKind,
    PackageName, PackageVersion, PlanStatus, ProxmoxEnvironment, ResolvedPackageChange, Scan,
    ScanId, ScanStatus, SecretId, Target, TargetId, UpdateClassification, UpdatePlan, UpdatePlanId,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::Database;

mod docker;
mod package_inventory;
pub use package_inventory::{PackageInventoryStatus, PersistedPackageInventory};
mod schedules;
mod targets;
mod telemetry;
mod worker_heartbeats;
pub use ansible_jobs::AnsibleQueueMetrics;

mod environment;

/// Fehler der Repository-Schicht ohne sensible SQL- oder Verbindungsdetails.
#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("database operation failed")]
    Database(#[source] sqlx::Error),

    #[error("invalid persisted value for {field}")]
    InvalidValue { field: &'static str },

    #[error("could not serialize persisted value")]
    Serialization(#[source] serde_json::Error),
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// Repository für Proxmox-Nodes.
#[derive(Clone)]
pub struct NodeRepository {
    pool: PgPool,
}

/// Repository für credential-freie Proxmox-Umgebungskonfigurationen.
#[derive(Clone)]
pub struct EnvironmentRepository {
    pool: PgPool,
}

/// Persistiert Ansible-Jobs und deren audit-sichere Ereignisse.
#[derive(Clone)]
pub struct AnsibleJobRepository {
    pool: PgPool,
}

/// Tracks the most recent liveness signal emitted by each Ansible worker.
#[derive(Clone)]
pub struct WorkerHeartbeatRepository {
    pool: PgPool,
}
#[derive(Clone)]
pub struct TelemetryRepository {
    pool: PgPool,
}

/// Persistiert Agent-Registrierungen ohne Secret-Werte.
#[derive(Clone)]
pub struct AgentRegistrationRepository {
    pool: PgPool,
}

/// Persistiert plattformneutrale verwaltete Ziele.
#[derive(Clone)]
pub struct TargetRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct DockerWorkloadRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct DockerDiscoveryRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct PackageInventoryRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct ScheduleRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct ContainerRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct ScanRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct UpdatePlanRepository {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct ExecutionEvent {
    pub id: Uuid,
    pub execution_id: ExecutionId,
    pub sequence: i64,
    pub event_type: String,
    pub message: Option<String>,
    pub fields: Value,
    pub created_at: DateTime<Utc>,
}

/// Persistiertes, gekürztes Ergebnis einer Agentenausführung.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResultRecord {
    pub execution_id: ExecutionId,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub created_at: DateTime<Utc>,
}

/// Repository für Update-Ausführungen und deren Events.
#[derive(Clone)]
pub struct ExecutionRepository {
    pool: PgPool,
}

pub struct AuditEvent {
    pub id: Uuid,
    pub node_id: Option<NodeId>,
    pub container_id: Option<ContainerId>,
    pub plan_id: Option<UpdatePlanId>,
    pub execution_id: Option<ExecutionId>,
    pub event_type: String,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

/// Repository für unveränderliche Audit-Ereignisse.
#[derive(Clone)]
pub struct AuditEventRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct Repositories {
    pub docker_workloads: DockerWorkloadRepository,
    pub docker_discovery: DockerDiscoveryRepository,
    pub package_inventory: PackageInventoryRepository,
    pub schedules: ScheduleRepository,
    pub targets: TargetRepository,
    pub agent_registrations: AgentRegistrationRepository,
    pub ansible_jobs: AnsibleJobRepository,
    pub worker_heartbeats: WorkerHeartbeatRepository,
    pub telemetry: TelemetryRepository,
    pub environments: EnvironmentRepository,
    pub nodes: NodeRepository,
    pub containers: ContainerRepository,
    pub scans: ScanRepository,
    pub plans: UpdatePlanRepository,
    pub executions: ExecutionRepository,
    pub audit_events: AuditEventRepository,
}

impl Repositories {
    pub fn new(database: &Database) -> Self {
        Self {
            docker_workloads: DockerWorkloadRepository::new(database),
            docker_discovery: DockerDiscoveryRepository::new(database),
            package_inventory: PackageInventoryRepository::new(database),
            schedules: ScheduleRepository::new(database),
            targets: TargetRepository::new(database),
            agent_registrations: AgentRegistrationRepository::new(database),
            ansible_jobs: AnsibleJobRepository::new(database),
            worker_heartbeats: WorkerHeartbeatRepository::new(database),
            telemetry: TelemetryRepository::new(database),
            environments: EnvironmentRepository::new(database),
            nodes: NodeRepository::new(database),
            containers: ContainerRepository::new(database),
            scans: ScanRepository::new(database),
            plans: UpdatePlanRepository::new(database),
            executions: ExecutionRepository::new(database),
            audit_events: AuditEventRepository::new(database),
        }
    }
}

fn is_active_ansible_job_status(status: &str) -> bool {
    matches!(
        status,
        "checking" | "planned" | "applying" | "reconcile_required"
    )
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn queued_jobs_do_not_block_a_new_worker_run() {
        assert!(!is_active_ansible_job_status("queued"));
        assert!(!is_active_ansible_job_status("succeeded"));
        assert!(is_active_ansible_job_status("applying"));
    }

    #[test]
    fn database_enum_mappings_cover_every_supported_value_and_reject_unknown_values() {
        assert_eq!(
            agent_state_to_db(lxcup_core::AgentConnectionState::Connected),
            "connected"
        );
        assert_eq!(
            agent_state_to_db(lxcup_core::AgentConnectionState::Degraded),
            "degraded"
        );
        assert_eq!(
            agent_state_to_db(lxcup_core::AgentConnectionState::Unreachable),
            "unreachable"
        );

        for status in [
            AnsibleJobStatus::Queued,
            AnsibleJobStatus::Checking,
            AnsibleJobStatus::Planned,
            AnsibleJobStatus::Applying,
            AnsibleJobStatus::ReconcileRequired,
            AnsibleJobStatus::Succeeded,
            AnsibleJobStatus::Failed,
            AnsibleJobStatus::Aborted,
        ] {
            assert!(!ansible_status_to_db(status).is_empty());
        }

        for value in ["configured", "connected", "failed", "disabled"] {
            assert!(environment_status_from_db(value.to_owned()).is_ok());
        }
        assert!(environment_status_from_db("other".to_owned()).is_err());

        for value in ["unknown", "connected", "disconnected"] {
            assert!(node_status_from_db(value.to_owned()).is_ok());
        }
        assert!(node_status_from_db("other".to_owned()).is_err());

        for value in ["debian", "ubuntu", "unknown:alpine", "fedora"] {
            assert!(matches!(
                os_from_db(value.to_owned()),
                OperatingSystem::Unknown(_) | OperatingSystem::Debian | OperatingSystem::Ubuntu
            ));
        }

        for value in ["running", "stopped", "unknown"] {
            assert!(container_status_from_db(value.to_owned()).is_ok());
        }
        assert!(container_status_from_db("other".to_owned()).is_err());

        for value in ["discovered", "managed", "ignored", "disabled"] {
            assert!(container_management_from_db(value.to_owned()).is_ok());
        }
        assert!(container_management_from_db("other".to_owned()).is_err());

        for value in ["pending", "running", "succeeded", "failed"] {
            assert!(scan_status_from_db(value.to_owned()).is_ok());
        }
        assert!(scan_status_from_db("other".to_owned()).is_err());

        for value in ["security", "normal", "unknown"] {
            assert!(classification_from_db(value.to_owned()).is_ok());
        }
        assert!(classification_from_db("other".to_owned()).is_err());

        for value in ["draft", "blocked", "ready", "confirmed", "invalidated"] {
            assert!(plan_status_from_db(value.to_owned()).is_ok());
        }
        assert!(plan_status_from_db("other".to_owned()).is_err());

        for value in ["install", "upgrade", "remove", "downgrade"] {
            assert!(change_kind_from_db(value.to_owned()).is_ok());
        }
        assert!(change_kind_from_db("other".to_owned()).is_err());

        for value in [
            "queued",
            "running",
            "succeeded",
            "failed",
            "aborted",
            "unknown",
        ] {
            assert!(execution_status_from_db(value.to_owned()).is_ok());
        }
        assert!(execution_status_from_db("other".to_owned()).is_err());
    }

    #[test]
    fn scalar_mappings_preserve_canonical_values() {
        assert_eq!(os_to_db(&OperatingSystem::Debian), "debian");
        assert_eq!(os_to_db(&OperatingSystem::Ubuntu), "ubuntu");
        assert_eq!(
            os_to_db(&OperatingSystem::Unknown("alpine".to_owned())),
            "unknown:alpine"
        );
        assert_eq!(container_id(42).unwrap().value(), 42);
        assert!(container_id(-1).is_err());
        assert_eq!(package_name("nginx".to_owned()).unwrap().as_str(), "nginx");
        assert!(package_name("".to_owned()).is_err());
        assert_eq!(
            package_version("1.2.3".to_owned()).unwrap().as_str(),
            "1.2.3"
        );
        assert!(package_version("".to_owned()).is_err());
    }
}

fn docker_workload_from_row(row: PgRow) -> Result<DockerWorkload, RepositoryError> {
    Ok(DockerWorkload {
        host_container_id: ContainerId::new(row.try_get::<i64, _>("host_container_id")? as u64),
        id: row.try_get("docker_id")?,
        name: row.try_get("name")?,
        image: row.try_get("image")?,
        state: row.try_get("state")?,
        status: row.try_get("status")?,
        ports: serde_json::from_value(row.try_get("ports")?)
            .map_err(RepositoryError::Serialization)?,
        started_at: row.try_get("started_at")?,
        labels: serde_json::from_value(row.try_get("labels")?)
            .map_err(RepositoryError::Serialization)?,
        presence: match row.try_get::<String, _>("presence")?.as_str() {
            "present" => lxcup_core::DockerWorkloadPresence::Present,
            "missing" => lxcup_core::DockerWorkloadPresence::Missing,
            _ => {
                return Err(RepositoryError::InvalidValue {
                    field: "docker workload presence",
                });
            }
        },
        change: match row.try_get::<String, _>("change_state")?.as_str() {
            "new" => lxcup_core::DockerWorkloadChange::New,
            "changed" => lxcup_core::DockerWorkloadChange::Changed,
            "unchanged" => lxcup_core::DockerWorkloadChange::Unchanged,
            "missing" => lxcup_core::DockerWorkloadChange::Missing,
            _ => {
                return Err(RepositoryError::InvalidValue {
                    field: "docker workload change state",
                });
            }
        },
        management_state: match row.try_get::<String, _>("management_state")?.as_str() {
            "discovered" => DockerWorkloadManagementState::Discovered,
            "managed" => DockerWorkloadManagementState::Managed,
            _ => {
                return Err(RepositoryError::InvalidValue {
                    field: "docker workload management state",
                });
            }
        },
        discovered_at: row.try_get("discovered_at")?,
    })
}

fn target_from_row(row: PgRow) -> Result<Target, RepositoryError> {
    serde_json::from_value(row.try_get("payload")?).map_err(|_| RepositoryError::InvalidValue {
        field: "target payload",
    })
}

fn agent_state_to_db(value: lxcup_core::AgentConnectionState) -> &'static str {
    match value {
        lxcup_core::AgentConnectionState::Connected => "connected",
        lxcup_core::AgentConnectionState::Degraded => "degraded",
        lxcup_core::AgentConnectionState::Unreachable => "unreachable",
    }
}

fn ansible_status_to_db(value: AnsibleJobStatus) -> &'static str {
    match value {
        AnsibleJobStatus::Queued => "queued",
        AnsibleJobStatus::Checking => "checking",
        AnsibleJobStatus::Planned => "planned",
        AnsibleJobStatus::Applying => "applying",
        AnsibleJobStatus::ReconcileRequired => "reconcile_required",
        AnsibleJobStatus::Succeeded => "succeeded",
        AnsibleJobStatus::Failed => "failed",
        AnsibleJobStatus::Aborted => "aborted",
    }
}

fn environment_from_row(row: PgRow) -> Result<ProxmoxEnvironment, RepositoryError> {
    Ok(ProxmoxEnvironment {
        id: lxcup_core::EnvironmentId::from_uuid(row.try_get("id")?),
        name: row.try_get("name")?,
        endpoint: row.try_get("endpoint")?,
        api_secret_ref: SecretId::from_uuid(row.try_get("api_secret_ref")?),
        ca_secret_ref: row
            .try_get::<Option<Uuid>, _>("ca_secret_ref")?
            .map(SecretId::from_uuid),
        status: environment_status_from_db(row.try_get("status")?)?,
        last_checked_at: row.try_get("last_checked_at")?,
        last_check_error: row.try_get("last_check_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn environment_status_to_db(value: EnvironmentStatus) -> &'static str {
    match value {
        EnvironmentStatus::Configured => "configured",
        EnvironmentStatus::Connected => "connected",
        EnvironmentStatus::Failed => "failed",
        EnvironmentStatus::Disabled => "disabled",
    }
}

fn environment_status_from_db(value: String) -> Result<EnvironmentStatus, RepositoryError> {
    match value.as_str() {
        "configured" => Ok(EnvironmentStatus::Configured),
        "connected" => Ok(EnvironmentStatus::Connected),
        "failed" => Ok(EnvironmentStatus::Failed),
        "disabled" => Ok(EnvironmentStatus::Disabled),
        _ => Err(RepositoryError::InvalidValue {
            field: "environment status",
        }),
    }
}

async fn save_scan(
    transaction: &mut Transaction<'_, Postgres>,
    scan: &Scan,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO scans (id, container_id, status, created_at, completed_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(scan.id.as_uuid())
    .bind(scan.container_id.value() as i64)
    .bind(scan_status_to_db(scan.status))
    .bind(scan.created_at)
    .bind(scan.completed_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn node_from_row(row: PgRow) -> Result<Node, RepositoryError> {
    Ok(Node {
        id: NodeId::from_uuid(row.try_get("id")?),
        name: row.try_get("name")?,
        address: row.try_get("address")?,
        status: node_status_from_db(row.try_get("status")?)?,
        proxmox_version: row.try_get("proxmox_version")?,
        capabilities: serde_json::from_value(row.try_get("capabilities")?).map_err(|_| {
            RepositoryError::InvalidValue {
                field: "node capabilities",
            }
        })?,
        last_checked_at: row.try_get("last_checked_at")?,
        last_check_error: row.try_get("last_check_error")?,
        created_at: row.try_get("created_at")?,
    })
}

fn container_from_row(row: PgRow) -> Result<Container, RepositoryError> {
    Ok(Container {
        id: container_id(row.try_get("id")?)?,
        node_id: NodeId::from_uuid(row.try_get("node_id")?),
        name: row.try_get("name")?,
        operating_system: os_from_db(row.try_get("operating_system")?),
        status: container_status_from_db(row.try_get("status")?)?,
        management_state: container_management_from_db(row.try_get("management_state")?)?,
        discovered_at: row.try_get("discovered_at")?,
    })
}

fn scan_from_row(row: PgRow) -> Result<Scan, RepositoryError> {
    Ok(Scan {
        id: ScanId::from_uuid(row.try_get("id")?),
        container_id: container_id(row.try_get("container_id")?)?,
        status: scan_status_from_db(row.try_get("status")?)?,
        updates: Vec::new(),
        created_at: row.try_get("created_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn available_update_from_row(row: PgRow) -> Result<AvailableUpdate, RepositoryError> {
    Ok(AvailableUpdate {
        package: package_name(row.try_get("package_name")?)?,
        installed_version: package_version(row.try_get("installed_version")?)?,
        candidate_version: package_version(row.try_get("candidate_version")?)?,
        classification: classification_from_db(row.try_get("classification")?)?,
        held: row.try_get("held")?,
    })
}

fn plan_from_row(row: PgRow) -> Result<UpdatePlan, RepositoryError> {
    Ok(UpdatePlan {
        id: UpdatePlanId::from_uuid(row.try_get("id")?),
        container_id: container_id(row.try_get("container_id")?)?,
        requested_packages: Vec::new(),
        resolved_changes: Vec::new(),
        status: plan_status_from_db(row.try_get("status")?)?,
        created_at: row.try_get("created_at")?,
        plan_hash: row.try_get("plan_hash")?,
    })
}

fn resolved_change_from_row(row: PgRow) -> Result<ResolvedPackageChange, RepositoryError> {
    Ok(ResolvedPackageChange {
        package: package_name(row.try_get("package_name")?)?,
        from_version: row
            .try_get::<Option<String>, _>("from_version")?
            .map(package_version)
            .transpose()?,
        to_version: row
            .try_get::<Option<String>, _>("to_version")?
            .map(package_version)
            .transpose()?,
        kind: change_kind_from_db(row.try_get("kind")?)?,
    })
}

fn execution_from_row(row: PgRow) -> Result<Execution, RepositoryError> {
    Ok(Execution {
        id: ExecutionId::from_uuid(row.try_get("id")?),
        plan_id: UpdatePlanId::from_uuid(row.try_get("plan_id")?),
        status: execution_status_from_db(row.try_get("status")?)?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

fn container_id(value: i64) -> Result<ContainerId, RepositoryError> {
    u64::try_from(value)
        .map(ContainerId::new)
        .map_err(|_| RepositoryError::InvalidValue {
            field: "container id",
        })
}

fn package_name(value: String) -> Result<PackageName, RepositoryError> {
    PackageName::new(value).map_err(|_| RepositoryError::InvalidValue {
        field: "package name",
    })
}

fn package_version(value: String) -> Result<PackageVersion, RepositoryError> {
    PackageVersion::new(value).map_err(|_| RepositoryError::InvalidValue {
        field: "package version",
    })
}

fn node_status_to_db(value: NodeStatus) -> &'static str {
    match value {
        NodeStatus::Unknown => "unknown",
        NodeStatus::Connected => "connected",
        NodeStatus::Disconnected => "disconnected",
    }
}

fn node_status_from_db(value: String) -> Result<NodeStatus, RepositoryError> {
    match value.as_str() {
        "unknown" => Ok(NodeStatus::Unknown),
        "connected" => Ok(NodeStatus::Connected),
        "disconnected" => Ok(NodeStatus::Disconnected),
        _ => Err(RepositoryError::InvalidValue {
            field: "node status",
        }),
    }
}

fn os_to_db(value: &OperatingSystem) -> String {
    match value {
        OperatingSystem::Debian => "debian".to_owned(),
        OperatingSystem::Ubuntu => "ubuntu".to_owned(),
        OperatingSystem::Unknown(value) => format!("unknown:{value}"),
    }
}

fn os_from_db(value: String) -> OperatingSystem {
    match value.as_str() {
        "debian" => OperatingSystem::Debian,
        "ubuntu" => OperatingSystem::Ubuntu,
        value if value.starts_with("unknown:") => OperatingSystem::Unknown(value[8..].to_owned()),
        value => OperatingSystem::Unknown(value.to_owned()),
    }
}

fn container_status_to_db(value: ContainerStatus) -> &'static str {
    match value {
        ContainerStatus::Running => "running",
        ContainerStatus::Stopped => "stopped",
        ContainerStatus::Unknown => "unknown",
    }
}

fn container_status_from_db(value: String) -> Result<ContainerStatus, RepositoryError> {
    match value.as_str() {
        "running" => Ok(ContainerStatus::Running),
        "stopped" => Ok(ContainerStatus::Stopped),
        "unknown" => Ok(ContainerStatus::Unknown),
        _ => Err(RepositoryError::InvalidValue {
            field: "container status",
        }),
    }
}

fn container_management_to_db(value: ContainerManagementState) -> &'static str {
    match value {
        ContainerManagementState::Discovered => "discovered",
        ContainerManagementState::Managed => "managed",
        ContainerManagementState::Ignored => "ignored",
        ContainerManagementState::Disabled => "disabled",
    }
}

fn container_management_from_db(
    value: String,
) -> Result<ContainerManagementState, RepositoryError> {
    match value.as_str() {
        "discovered" => Ok(ContainerManagementState::Discovered),
        "managed" => Ok(ContainerManagementState::Managed),
        "ignored" => Ok(ContainerManagementState::Ignored),
        "disabled" => Ok(ContainerManagementState::Disabled),
        _ => Err(RepositoryError::InvalidValue {
            field: "container management state",
        }),
    }
}

fn scan_status_to_db(value: ScanStatus) -> &'static str {
    match value {
        ScanStatus::Pending => "pending",
        ScanStatus::Running => "running",
        ScanStatus::Succeeded => "succeeded",
        ScanStatus::Failed => "failed",
    }
}

fn scan_status_from_db(value: String) -> Result<ScanStatus, RepositoryError> {
    match value.as_str() {
        "pending" => Ok(ScanStatus::Pending),
        "running" => Ok(ScanStatus::Running),
        "succeeded" => Ok(ScanStatus::Succeeded),
        "failed" => Ok(ScanStatus::Failed),
        _ => Err(RepositoryError::InvalidValue {
            field: "scan status",
        }),
    }
}

fn classification_to_db(value: UpdateClassification) -> &'static str {
    match value {
        UpdateClassification::Security => "security",
        UpdateClassification::Normal => "normal",
        UpdateClassification::Unknown => "unknown",
    }
}

fn classification_from_db(value: String) -> Result<UpdateClassification, RepositoryError> {
    match value.as_str() {
        "security" => Ok(UpdateClassification::Security),
        "normal" => Ok(UpdateClassification::Normal),
        "unknown" => Ok(UpdateClassification::Unknown),
        _ => Err(RepositoryError::InvalidValue {
            field: "update classification",
        }),
    }
}

fn plan_status_to_db(value: PlanStatus) -> &'static str {
    match value {
        PlanStatus::Draft => "draft",
        PlanStatus::Blocked => "blocked",
        PlanStatus::Ready => "ready",
        PlanStatus::Confirmed => "confirmed",
        PlanStatus::Invalidated => "invalidated",
    }
}

fn plan_status_from_db(value: String) -> Result<PlanStatus, RepositoryError> {
    match value.as_str() {
        "draft" => Ok(PlanStatus::Draft),
        "blocked" => Ok(PlanStatus::Blocked),
        "ready" => Ok(PlanStatus::Ready),
        "confirmed" => Ok(PlanStatus::Confirmed),
        "invalidated" => Ok(PlanStatus::Invalidated),
        _ => Err(RepositoryError::InvalidValue {
            field: "plan status",
        }),
    }
}

fn change_kind_to_db(value: PackageChangeKind) -> &'static str {
    match value {
        PackageChangeKind::Install => "install",
        PackageChangeKind::Upgrade => "upgrade",
        PackageChangeKind::Remove => "remove",
        PackageChangeKind::Downgrade => "downgrade",
    }
}

fn change_kind_from_db(value: String) -> Result<PackageChangeKind, RepositoryError> {
    match value.as_str() {
        "install" => Ok(PackageChangeKind::Install),
        "upgrade" => Ok(PackageChangeKind::Upgrade),
        "remove" => Ok(PackageChangeKind::Remove),
        "downgrade" => Ok(PackageChangeKind::Downgrade),
        _ => Err(RepositoryError::InvalidValue {
            field: "package change kind",
        }),
    }
}

fn execution_status_to_db(value: ExecutionStatus) -> &'static str {
    match value {
        ExecutionStatus::Queued => "queued",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Aborted => "aborted",
        ExecutionStatus::Unknown => "unknown",
    }
}

fn execution_status_from_db(value: String) -> Result<ExecutionStatus, RepositoryError> {
    match value.as_str() {
        "queued" => Ok(ExecutionStatus::Queued),
        "running" => Ok(ExecutionStatus::Running),
        "succeeded" => Ok(ExecutionStatus::Succeeded),
        "failed" => Ok(ExecutionStatus::Failed),
        "aborted" => Ok(ExecutionStatus::Aborted),
        "unknown" => Ok(ExecutionStatus::Unknown),
        _ => Err(RepositoryError::InvalidValue {
            field: "execution status",
        }),
    }
}
mod agent_registration;
mod ansible_jobs;
mod audit;
mod containers;
mod executions;
mod nodes;
mod plans;
mod scans;
