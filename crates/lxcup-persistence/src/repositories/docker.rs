use super::*;

impl DockerWorkloadRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }
    pub async fn upsert_discovered(&self, item: &DockerWorkload) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO docker_workloads (host_container_id, docker_id, name, image, state, status, management_state, discovered_at) VALUES ($1,$2,$3,$4,$5,$6,'discovered',$7) ON CONFLICT (host_container_id, docker_id) DO UPDATE SET name=EXCLUDED.name,image=EXCLUDED.image,state=EXCLUDED.state,status=EXCLUDED.status,discovered_at=EXCLUDED.discovered_at").bind(item.host_container_id.value() as i64).bind(&item.id).bind(&item.name).bind(&item.image).bind(&item.state).bind(&item.status).bind(item.discovered_at).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn list(&self, host: ContainerId) -> Result<Vec<DockerWorkload>, RepositoryError> {
        let rows = sqlx::query("SELECT host_container_id, docker_id, name, image, state, status, management_state, discovered_at FROM docker_workloads WHERE host_container_id=$1 ORDER BY name").bind(host.value() as i64).fetch_all(&self.pool).await?;
        rows.into_iter().map(docker_workload_from_row).collect()
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
