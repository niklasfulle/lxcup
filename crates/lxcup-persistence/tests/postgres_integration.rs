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
use sqlx::PgPool;
use uuid::Uuid;

fn postgres_now() -> chrono::DateTime<Utc> {
    let now = Utc::now();
    now.with_nanosecond((now.nanosecond() / 1_000) * 1_000)
        .expect("valid microsecond timestamp")
}

struct ScopedTestDatabase {
    database: Database,
    admin_pool: PgPool,
    schema: String,
}

impl ScopedTestDatabase {
    fn database(&self) -> &Database {
        &self.database
    }

    async fn finish(self) {
        self.database.pool().close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin_pool)
            .await
            .unwrap();
        self.admin_pool.close().await;
    }
}

async fn scoped_test_database(test_name: &str) -> Option<ScopedTestDatabase> {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return None;
    };
    let schema = format!("it_{}_{}", test_name, Uuid::new_v4().simple());
    let admin_pool = PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = DatabaseConfig::from_values(
        scoped_url,
        3,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    Some(ScopedTestDatabase {
        database,
        admin_pool,
        schema,
    })
}

#[path = "postgres_integration/ansible_jobs.rs"]
mod ansible_jobs;
#[path = "postgres_integration/auth_users.rs"]
mod auth_users;
#[path = "postgres_integration/round_trip.rs"]
mod round_trip;
#[path = "postgres_integration/target_inventory.rs"]
mod target_inventory;
#[path = "postgres_integration/target_removal.rs"]
mod target_removal;

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
