use std::time::Duration;

use chrono::{Timelike, Utc};
use lxcup_agent::{
    AgentHeartbeat, AgentInfo, AgentMetrics, AgentPlatform, SystemTelemetrySample,
    SystemTelemetryWindow,
};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters,
    ExecutionMode, JobEvent, JobEventKind,
};
use lxcup_core::{
    ActorRole, AgentRegistration, Container, ContainerId, ContainerStatus, DockerWorkload,
    DockerWorkloadManagementState, Execution, ExecutionStatus, InstalledPackage, JobSchedule, Node,
    OperatingSystem, PackageInventorySnapshot, PackageName, PackageVersion, ResourceLifecycle,
    ResourceTarget, ScheduleFrequency, SecretId, Target, TargetKind, TargetTransport, UpdatePolicy,
    UpdateRisk,
};
use lxcup_persistence::{
    AuditEvent, Database, DatabaseConfig, ExecutionEvent, Repositories, seeds::seed_development,
};
use serde_json::json;
use uuid::Uuid;

fn postgres_now() -> chrono::DateTime<Utc> {
    let now = Utc::now();
    now.with_nanosecond((now.nanosecond() / 1_000) * 1_000)
        .expect("valid microsecond timestamp")
}

#[path = "postgres_integration/ansible_jobs.rs"]
mod ansible_jobs;
#[path = "postgres_integration/round_trip.rs"]
mod round_trip;
#[path = "postgres_integration/target_inventory.rs"]
mod target_inventory;

async fn cleanup(
    database: &Database,
    audit_id: Uuid,
    event_id: Uuid,
    execution_id: lxcup_core::ExecutionId,
    plan_id: lxcup_core::UpdatePlanId,
    scan_id: lxcup_core::ScanId,
) -> Result<(), sqlx::Error> {
    let pool = database.pool();
    sqlx::query("DELETE FROM audit_events WHERE id = $1")
        .bind(audit_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM execution_events WHERE id = $1")
        .bind(event_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM executions WHERE id = $1")
        .bind(execution_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plan_changes WHERE plan_id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plan_requested_packages WHERE plan_id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM update_plans WHERE id = $1")
        .bind(plan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM scan_updates WHERE scan_id = $1")
        .bind(scan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM scans WHERE id = $1")
        .bind(scan_id.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM containers WHERE id = $1")
        .bind(101_i64)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(Uuid::from_u128(0x10000000000000000000000000000001))
        .execute(pool)
        .await?;
    Ok(())
}
