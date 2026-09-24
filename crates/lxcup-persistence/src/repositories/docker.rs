use super::{
    ContainerId, Database, DockerDiscoveryRepository, DockerWorkload, DockerWorkloadRepository,
    RepositoryError, docker_workload_from_row,
};
use chrono::Utc;
use lxcup_core::{DockerDiscoveryRun, DockerDiscoveryStatus, DockerWorkloadChange};
use sqlx::Row;
use uuid::Uuid;

impl DockerWorkloadRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }
    pub async fn upsert_discovered(
        &self,
        item: &DockerWorkload,
    ) -> Result<DockerWorkloadChange, RepositoryError> {
        let changed: String = sqlx::query_scalar("INSERT INTO docker_workloads (host_container_id, docker_id, name, image, state, status, management_state, discovered_at, ports, started_at, labels, presence, last_seen_at, change_state) VALUES ($1,$2,$3,$4,$5,$6,'discovered',$7,$8,$9,$10,'present',$7,'new') ON CONFLICT (host_container_id, docker_id) DO UPDATE SET change_state=CASE WHEN docker_workloads.presence='missing' OR docker_workloads.name IS DISTINCT FROM EXCLUDED.name OR docker_workloads.image IS DISTINCT FROM EXCLUDED.image OR docker_workloads.state IS DISTINCT FROM EXCLUDED.state OR docker_workloads.status IS DISTINCT FROM EXCLUDED.status OR docker_workloads.ports IS DISTINCT FROM EXCLUDED.ports OR docker_workloads.started_at IS DISTINCT FROM EXCLUDED.started_at OR docker_workloads.labels IS DISTINCT FROM EXCLUDED.labels THEN 'changed' ELSE 'unchanged' END,name=EXCLUDED.name,image=EXCLUDED.image,state=EXCLUDED.state,status=EXCLUDED.status,ports=EXCLUDED.ports,started_at=EXCLUDED.started_at,labels=EXCLUDED.labels,presence='present',discovered_at=EXCLUDED.discovered_at,last_seen_at=EXCLUDED.last_seen_at RETURNING change_state")
            .bind(item.host_container_id.value() as i64)
            .bind(&item.id)
            .bind(&item.name)
            .bind(&item.image)
            .bind(&item.state)
            .bind(&item.status)
            .bind(item.discovered_at)
            .bind(serde_json::to_value(&item.ports).map_err(RepositoryError::Serialization)?)
            .bind(&item.started_at)
            .bind(serde_json::to_value(&item.labels).map_err(RepositoryError::Serialization)?)
            .fetch_one(&self.pool)
            .await?;
        match changed.as_str() {
            "new" => Ok(DockerWorkloadChange::New),
            "changed" => Ok(DockerWorkloadChange::Changed),
            "unchanged" => Ok(DockerWorkloadChange::Unchanged),
            _ => Err(RepositoryError::InvalidValue {
                field: "docker workload change state",
            }),
        }
    }
    pub async fn list(&self, host: ContainerId) -> Result<Vec<DockerWorkload>, RepositoryError> {
        let rows = sqlx::query("SELECT host_container_id, docker_id, name, image, state, status, management_state, discovered_at, ports, started_at, labels, presence, change_state FROM docker_workloads WHERE host_container_id=$1 ORDER BY name").bind(host.value() as i64).fetch_all(&self.pool).await?;
        rows.into_iter().map(docker_workload_from_row).collect()
    }
    pub async fn mark_missing_except(
        &self,
        host: ContainerId,
        seen_ids: &[String],
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE docker_workloads SET presence='missing', change_state='missing' WHERE host_container_id=$1 AND presence='present' AND NOT (docker_id = ANY($2))")
            .bind(host.value() as i64).bind(seen_ids).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn adopt(&self, host: ContainerId, id: &str) -> Result<bool, RepositoryError> {
        Ok(sqlx::query("UPDATE docker_workloads SET management_state='managed' WHERE host_container_id=$1 AND docker_id=$2").bind(host.value() as i64).bind(id).execute(&self.pool).await?.rows_affected() == 1)
    }
    pub async fn remove(&self, host: ContainerId, id: &str) -> Result<bool, RepositoryError> {
        Ok(
            sqlx::query("DELETE FROM docker_workloads WHERE host_container_id=$1 AND docker_id=$2")
                .bind(host.value() as i64)
                .bind(id)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }
}

impl DockerDiscoveryRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn start(&self, host: ContainerId) -> Result<DockerDiscoveryRun, RepositoryError> {
        let run = DockerDiscoveryRun {
            id: Uuid::new_v4(),
            host_container_id: host,
            status: DockerDiscoveryStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            container_count: 0,
            error_code: None,
        };
        sqlx::query("INSERT INTO docker_discovery_runs (id, host_container_id, status, started_at, container_count) VALUES ($1,$2,'running',$3,0)")
            .bind(run.id)
            .bind(host.value() as i64)
            .bind(run.started_at)
            .execute(&self.pool)
            .await?;
        Ok(run)
    }

    pub async fn finish(
        &self,
        id: Uuid,
        status: DockerDiscoveryStatus,
        container_count: u32,
        error_code: Option<&str>,
    ) -> Result<DockerDiscoveryRun, RepositoryError> {
        let status_name = match status {
            DockerDiscoveryStatus::Running => "running",
            DockerDiscoveryStatus::Succeeded => "succeeded",
            DockerDiscoveryStatus::Failed => "failed",
        };
        let row = sqlx::query("UPDATE docker_discovery_runs SET status=$2, finished_at=NOW(), container_count=$3, error_code=$4 WHERE id=$1 RETURNING id, host_container_id, status, started_at, finished_at, container_count, error_code")
            .bind(id)
            .bind(status_name)
            .bind(container_count as i32)
            .bind(error_code)
            .fetch_one(&self.pool)
            .await?;
        discovery_run_from_row(&row)
    }

    pub async fn latest(
        &self,
        host: ContainerId,
    ) -> Result<Option<DockerDiscoveryRun>, RepositoryError> {
        let row = sqlx::query("SELECT id, host_container_id, status, started_at, finished_at, container_count, error_code FROM docker_discovery_runs WHERE host_container_id=$1 ORDER BY started_at DESC LIMIT 1")
            .bind(host.value() as i64)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(discovery_run_from_row).transpose()
    }
}

fn discovery_run_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DockerDiscoveryRun, RepositoryError> {
    Ok(DockerDiscoveryRun {
        id: row.try_get("id")?,
        host_container_id: ContainerId::new(row.try_get::<i64, _>("host_container_id")? as u64),
        status: match row.try_get::<String, _>("status")?.as_str() {
            "running" => DockerDiscoveryStatus::Running,
            "succeeded" => DockerDiscoveryStatus::Succeeded,
            "failed" => DockerDiscoveryStatus::Failed,
            _ => {
                return Err(RepositoryError::InvalidValue {
                    field: "docker discovery status",
                });
            }
        },
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        container_count: row.try_get::<i32, _>("container_count")? as u32,
        error_code: row.try_get("error_code")?,
    })
}
