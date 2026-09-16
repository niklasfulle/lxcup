//! SQL-Repositorys für die MVP-Domain.

use chrono::{DateTime, Utc};
use lxcup_core::{
    AvailableUpdate, Container, ContainerId, ContainerManagementState, ContainerStatus, Execution,
    ExecutionId, ExecutionStatus, Node, NodeId, NodeStatus, OperatingSystem, PackageChangeKind,
    PackageName, PackageVersion, PlanStatus, ResolvedPackageChange, Scan, ScanId, ScanStatus,
    UpdateClassification, UpdatePlan, UpdatePlanId,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::Database;

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

impl NodeRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, node: &Node) -> Result<(), RepositoryError> {
        let capabilities =
            serde_json::to_value(&node.capabilities).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO nodes (id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(node.id.as_uuid())
        .bind(&node.name)
        .bind(&node.address)
        .bind(node_status_to_db(node.status))
        .bind(node.proxmox_version.as_deref())
        .bind(capabilities)
        .bind(node.last_checked_at)
        .bind(node.last_check_error.as_deref())
        .bind(node.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, node: &Node) -> Result<(), RepositoryError> {
        let capabilities =
            serde_json::to_value(&node.capabilities).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "UPDATE nodes SET name = $1, address = $2, status = $3, proxmox_version = $4, capabilities = $5, last_checked_at = $6, last_check_error = $7, created_at = $8 WHERE id = $9",
        )
        .bind(&node.name)
        .bind(&node.address)
        .bind(node_status_to_db(node.status))
        .bind(node.proxmox_version.as_deref())
        .bind(capabilities)
        .bind(node.last_checked_at)
        .bind(node.last_check_error.as_deref())
        .bind(node.created_at)
        .bind(node.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: NodeId) -> Result<Option<Node>, RepositoryError> {
        let row =
            sqlx::query("SELECT id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at FROM nodes WHERE id = $1")
                .bind(id.as_uuid())
                .fetch_optional(&self.pool)
                .await?;

        row.map(node_from_row).transpose()
    }

    pub async fn list(&self) -> Result<Vec<Node>, RepositoryError> {
        let rows =
            sqlx::query("SELECT id, name, address, status, proxmox_version, capabilities, last_checked_at, last_check_error, created_at FROM nodes ORDER BY name")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter().map(node_from_row).collect()
    }
}

/// Repository für LXC-Container.
#[derive(Clone)]
pub struct ContainerRepository {
    pool: PgPool,
}

impl ContainerRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, container: &Container) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO containers (id, node_id, name, operating_system, status, management_state, discovered_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(container.id.value() as i64)
        .bind(container.node_id.as_uuid())
        .bind(&container.name)
        .bind(os_to_db(&container.operating_system))
        .bind(container_status_to_db(container.status))
        .bind(container_management_to_db(container.management_state))
        .bind(container.discovered_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Inserts a newly discovered container or refreshes only its inventory data.
    /// The management state is intentionally preserved on refresh.
    pub async fn upsert_discovered(&self, container: &Container) -> Result<bool, RepositoryError> {
        let inserted = sqlx::query(
            "INSERT INTO containers (id, node_id, name, operating_system, status, management_state, discovered_at) VALUES ($1, $2, $3, $4, $5, 'discovered', $6) ON CONFLICT (id) DO NOTHING",
        )
        .bind(container.id.value() as i64)
        .bind(container.node_id.as_uuid())
        .bind(&container.name)
        .bind(os_to_db(&container.operating_system))
        .bind(container_status_to_db(container.status))
        .bind(container.discovered_at)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;

        if !inserted {
            sqlx::query(
                "UPDATE containers SET node_id = $1, name = $2, operating_system = $3, status = $4, discovered_at = $5 WHERE id = $6",
            )
            .bind(container.node_id.as_uuid())
            .bind(&container.name)
            .bind(os_to_db(&container.operating_system))
            .bind(container_status_to_db(container.status))
            .bind(container.discovered_at)
            .bind(container.id.value() as i64)
            .execute(&self.pool)
            .await?;
        }

        Ok(inserted)
    }

    pub async fn update_management_state(
        &self,
        container: &Container,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE containers SET management_state = $1 WHERE id = $2")
            .bind(container_management_to_db(container.management_state))
            .bind(container.id.value() as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_status_unknown(&self, id: ContainerId) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE containers SET status = 'unknown' WHERE id = $1")
            .bind(id.value() as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: ContainerId) -> Result<Option<Container>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, node_id, name, operating_system, status, management_state, discovered_at FROM containers WHERE id = $1",
        )
        .bind(id.value() as i64)
        .fetch_optional(&self.pool)
        .await?;

        row.map(container_from_row).transpose()
    }

    pub async fn list_by_node(&self, node_id: NodeId) -> Result<Vec<Container>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, node_id, name, operating_system, status, management_state, discovered_at FROM containers WHERE node_id = $1 ORDER BY id",
        )
        .bind(node_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(container_from_row).collect()
    }
}

/// Repository für Scan-Ergebnisse und erkannte Updates.
#[derive(Clone)]
pub struct ScanRepository {
    pool: PgPool,
}

impl ScanRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, scan: &Scan) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        save_scan(&mut transaction, scan).await?;
        for update in &scan.updates {
            sqlx::query(
                "INSERT INTO scan_updates (scan_id, package_name, installed_version, candidate_version, classification, held) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(scan.id.as_uuid())
            .bind(update.package.as_str())
            .bind(update.installed_version.as_str())
            .bind(update.candidate_version.as_str())
            .bind(classification_to_db(update.classification))
            .bind(update.held)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: ScanId) -> Result<Option<Scan>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT id, container_id, status, created_at, completed_at FROM scans WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let update_rows = sqlx::query(
            "SELECT package_name, installed_version, candidate_version, classification, held FROM scan_updates WHERE scan_id = $1 ORDER BY package_name",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        let mut scan = scan_from_row(row)?;
        scan.updates = update_rows
            .into_iter()
            .map(available_update_from_row)
            .collect::<Result<_, _>>()?;
        Ok(Some(scan))
    }
}

/// Repository für aufgelöste Update-Pläne.
#[derive(Clone)]
pub struct UpdatePlanRepository {
    pool: PgPool,
}

impl UpdatePlanRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, plan: &UpdatePlan) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO update_plans (id, container_id, status, created_at, plan_hash) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(plan.id.as_uuid())
        .bind(plan.container_id.value() as i64)
        .bind(plan_status_to_db(plan.status))
        .bind(plan.created_at)
        .bind(plan.plan_hash.as_deref())
        .execute(&mut *transaction)
        .await?;

        for package in &plan.requested_packages {
            sqlx::query(
                "INSERT INTO update_plan_requested_packages (plan_id, package_name) VALUES ($1, $2)",
            )
            .bind(plan.id.as_uuid())
            .bind(package.as_str())
            .execute(&mut *transaction)
            .await?;
        }
        for change in &plan.resolved_changes {
            sqlx::query(
                "INSERT INTO update_plan_changes (plan_id, package_name, from_version, to_version, kind) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(plan.id.as_uuid())
            .bind(change.package.as_str())
            .bind(change.from_version.as_ref().map(PackageVersion::as_str))
            .bind(change.to_version.as_ref().map(PackageVersion::as_str))
            .bind(change_kind_to_db(change.kind))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn find_by_id(
        &self,
        id: UpdatePlanId,
    ) -> Result<Option<UpdatePlan>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT id, container_id, status, created_at, plan_hash FROM update_plans WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let packages = sqlx::query(
            "SELECT package_name FROM update_plan_requested_packages WHERE plan_id = $1 ORDER BY package_name",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;
        let changes = sqlx::query(
            "SELECT package_name, from_version, to_version, kind FROM update_plan_changes WHERE plan_id = $1 ORDER BY package_name, kind",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        let mut plan = plan_from_row(row)?;
        plan.requested_packages = packages
            .into_iter()
            .map(|row| package_name(row.get("package_name")))
            .collect::<Result<_, _>>()?;
        plan.resolved_changes = changes
            .into_iter()
            .map(resolved_change_from_row)
            .collect::<Result<_, _>>()?;
        Ok(Some(plan))
    }

    /// Liefert die Planhistorie eines Containers chronologisch aufsteigend.
    pub async fn list_by_container(
        &self,
        container_id: ContainerId,
    ) -> Result<Vec<UpdatePlan>, RepositoryError> {
        let ids = sqlx::query(
            "SELECT id FROM update_plans WHERE container_id = $1 ORDER BY created_at, id",
        )
        .bind(container_id.value() as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut plans = Vec::with_capacity(ids.len());
        for row in ids {
            let id = UpdatePlanId::from_uuid(row.get("id"));
            if let Some(plan) = self.find_by_id(id).await? {
                plans.push(plan);
            }
        }
        Ok(plans)
    }
}

/// Einzelnes Execution-Event für Audit- und Reconciliation-Zwecke.
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

impl ExecutionRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, execution: &Execution) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO executions (id, plan_id, status, started_at, finished_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(execution.id.as_uuid())
        .bind(execution.plan_id.as_uuid())
        .bind(execution_status_to_db(execution.status))
        .bind(execution.started_at)
        .bind(execution.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, execution: &Execution) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE executions SET plan_id = $1, status = $2, started_at = $3, finished_at = $4 WHERE id = $5",
        )
        .bind(execution.plan_id.as_uuid())
        .bind(execution_status_to_db(execution.status))
        .bind(execution.started_at)
        .bind(execution.finished_at)
        .bind(execution.id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Claims an idempotency key exactly once for an execution.
    pub async fn claim_idempotency_key(
        &self,
        key: &str,
        execution_id: ExecutionId,
    ) -> Result<bool, RepositoryError> {
        let inserted = sqlx::query(
            "INSERT INTO execution_requests (idempotency_key, execution_id) VALUES ($1, $2) ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(key)
        .bind(execution_id.as_uuid())
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(inserted)
    }

    pub async fn find_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<ExecutionId>, RepositoryError> {
        let row =
            sqlx::query("SELECT execution_id FROM execution_requests WHERE idempotency_key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| ExecutionId::from_uuid(row.get("execution_id"))))
    }

    pub async fn find_by_id(&self, id: ExecutionId) -> Result<Option<Execution>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, plan_id, status, started_at, finished_at FROM executions WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;

        row.map(execution_from_row).transpose()
    }

    pub async fn append_event(&self, event: &ExecutionEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO execution_events (id, execution_id, sequence, event_type, message, fields, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(event.id)
        .bind(event.execution_id.as_uuid())
        .bind(event.sequence)
        .bind(&event.event_type)
        .bind(event.message.as_deref())
        .bind(&event.fields)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_result(&self, result: &ExecutionResultRecord) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO execution_results (execution_id, exit_code, stdout, stderr, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (execution_id) DO UPDATE SET exit_code = EXCLUDED.exit_code, stdout = EXCLUDED.stdout, stderr = EXCLUDED.stderr, created_at = EXCLUDED.created_at",
        )
        .bind(result.execution_id.as_uuid())
        .bind(result.exit_code)
        .bind(&result.stdout)
        .bind(&result.stderr)
        .bind(result.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_result(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Option<ExecutionResultRecord>, RepositoryError> {
        let row = sqlx::query("SELECT execution_id, exit_code, stdout, stderr, created_at FROM execution_results WHERE execution_id = $1")
            .bind(execution_id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(ExecutionResultRecord {
                execution_id: ExecutionId::from_uuid(row.try_get("execution_id")?),
                exit_code: row.try_get("exit_code")?,
                stdout: row.try_get("stdout")?,
                stderr: row.try_get("stderr")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .transpose()
    }
}

/// Audit-Ereignis über eine fachliche oder technische Aktion.
#[derive(Clone, Debug)]
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

impl AuditEventRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn append(&self, event: &AuditEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO audit_events (id, node_id, container_id, plan_id, execution_id, event_type, details, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(event.id)
        .bind(event.node_id.map(NodeId::as_uuid))
        .bind(event.container_id.map(|id| id.value() as i64))
        .bind(event.plan_id.map(UpdatePlanId::as_uuid))
        .bind(event.execution_id.map(ExecutionId::as_uuid))
        .bind(&event.event_type)
        .bind(&event.details)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Erstellt alle MVP-Repositories über denselben Pool.
#[derive(Clone)]
pub struct Repositories {
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
            nodes: NodeRepository::new(database),
            containers: ContainerRepository::new(database),
            scans: ScanRepository::new(database),
            plans: UpdatePlanRepository::new(database),
            executions: ExecutionRepository::new(database),
            audit_events: AuditEventRepository::new(database),
        }
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
