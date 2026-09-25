use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, ContainerId, DockerContainerInfo, DockerWorkload,
    DockerWorkloadManagementState, envelope, parse_container_id,
};
use axum::{
    Json,
    extract::{Json as JsonBody, Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct DockerWorkloadDto {
    pub host_container_id: ContainerId,
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<String>,
    pub started_at: Option<String>,
    pub labels: Vec<String>,
    pub presence: String,
    pub change_state: String,
    pub management_state: String,
    pub discovered_at: chrono::DateTime<chrono::Utc>,
}

impl DockerWorkloadDto {
    pub(super) fn discovered(
        host_container_id: ContainerId,
        container: DockerContainerInfo,
    ) -> Self {
        Self {
            host_container_id,
            id: container.id,
            name: container.name,
            image: container.image,
            state: container.state,
            status: container.status,
            ports: container.ports,
            started_at: container.started_at,
            labels: container.labels,
            presence: "present".to_owned(),
            change_state: "new".to_owned(),
            management_state: "discovered".to_owned(),
            discovered_at: chrono::Utc::now(),
        }
    }
}

impl From<DockerWorkload> for DockerWorkloadDto {
    fn from(item: DockerWorkload) -> Self {
        Self {
            host_container_id: item.host_container_id,
            id: item.id,
            name: item.name,
            image: item.image,
            state: item.state,
            status: item.status,
            ports: item.ports,
            started_at: item.started_at,
            labels: item.labels,
            presence: match item.presence {
                lxcup_core::DockerWorkloadPresence::Present => "present".to_owned(),
                lxcup_core::DockerWorkloadPresence::Missing => "missing".to_owned(),
            },
            change_state: match item.change {
                lxcup_core::DockerWorkloadChange::New => "new".to_owned(),
                lxcup_core::DockerWorkloadChange::Changed => "changed".to_owned(),
                lxcup_core::DockerWorkloadChange::Unchanged => "unchanged".to_owned(),
                lxcup_core::DockerWorkloadChange::Missing => "missing".to_owned(),
            },
            management_state: match item.management_state {
                DockerWorkloadManagementState::Discovered => "discovered".to_owned(),
                DockerWorkloadManagementState::Managed => "managed".to_owned(),
            },
            discovered_at: item.discovered_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DockerDiscoveryResultDto {
    pub run: lxcup_core::DockerDiscoveryRun,
    pub workloads: Vec<DockerWorkloadDto>,
}

pub(crate) async fn get_docker_discovery(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Option<lxcup_core::DockerDiscoveryRun>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let latest = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .docker_discovery
            .latest(container_id)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state
            .store
            .read()
            .await
            .docker_discovery_runs
            .iter()
            .rev()
            .find(|run| run.host_container_id == container_id)
            .cloned()
    };
    Ok(Json(envelope(latest)))
}

pub(crate) async fn list_docker_containers(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<DockerWorkloadDto>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    if let Some(repositories) = state.repositories.as_ref() {
        let workloads = repositories
            .docker_workloads
            .list(container_id)
            .await
            .map_err(|_| {
                ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht gelesen werden",
                )
            })?
            .into_iter()
            .map(DockerWorkloadDto::from)
            .collect();
        return Ok(Json(envelope(workloads)));
    }
    let workloads = state
        .store
        .read()
        .await
        .docker_workloads
        .values()
        .filter(|item| item.host_container_id == container_id)
        .cloned()
        .collect();
    Ok(Json(envelope(workloads)))
}

pub(crate) async fn discover_docker_containers(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<DockerDiscoveryResultDto>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let mut run = start_docker_discovery(&state, container_id).await?;
    state.publish(ApiEvent::status(
        "docker_discovery",
        container_id.value().to_string(),
        "running",
    ));

    let discovered = fetch_docker_inventory(&state, container_id, &mut run).await?;
    let container_count = discovered.containers.len() as u32;
    if let Some(repositories) = state.repositories.as_ref() {
        let workloads =
            persist_docker_inventory(&state, repositories, container_id, &discovered, &mut run)
                .await?;
        let run = finish_docker_discovery(&state, &mut run, container_count, None).await?;
        return Ok(Json(envelope(DockerDiscoveryResultDto { run, workloads })));
    }
    let workloads = reconcile_memory_docker_inventory(&state, container_id, discovered).await;
    let run = finish_docker_discovery(&state, &mut run, container_count, None).await?;
    Ok(Json(envelope(DockerDiscoveryResultDto { run, workloads })))
}

async fn start_docker_discovery(
    state: &ApiState,
    container_id: ContainerId,
) -> Result<lxcup_core::DockerDiscoveryRun, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        return repositories
            .docker_discovery
            .start(container_id)
            .await
            .map_err(|_| ApiError::storage());
    }
    let run = lxcup_core::DockerDiscoveryRun {
        id: uuid::Uuid::new_v4(),
        host_container_id: container_id,
        status: lxcup_core::DockerDiscoveryStatus::Running,
        started_at: chrono::Utc::now(),
        finished_at: None,
        container_count: 0,
        error_code: None,
    };
    state
        .store
        .write()
        .await
        .docker_discovery_runs
        .push(run.clone());
    Ok(run)
}

async fn fetch_docker_inventory(
    state: &ApiState,
    container_id: ContainerId,
    run: &mut lxcup_core::DockerDiscoveryRun,
) -> Result<lxcup_agent::DockerDiscovery, ApiError> {
    let agent = state.agents.read().await.get(&container_id).cloned();
    let Some(agent) = agent else {
        finish_docker_discovery(state, run, 0, Some("agent_unavailable")).await?;
        return Err(ApiError::not_found("agent not registered"));
    };
    let discovered = match agent.client.docker_containers().await {
        Ok(discovered) => discovered,
        Err(_) => {
            finish_docker_discovery(state, run, 0, Some("docker_discovery_failed")).await?;
            return Err(ApiError::dependency(
                "docker_discovery_failed",
                "Docker-Inventar konnte vom Agenten nicht gelesen werden",
            ));
        }
    };
    if !discovered.available {
        finish_docker_discovery(state, run, 0, Some("docker_unavailable")).await?;
        return Err(ApiError::dependency(
            "docker_unavailable",
            "Auf diesem Host ist Docker nicht verfügbar; bestehende Funde bleiben unverändert",
        ));
    }
    Ok(discovered)
}

async fn persist_docker_inventory(
    state: &ApiState,
    repositories: &lxcup_persistence::Repositories,
    container_id: ContainerId,
    discovered: &lxcup_agent::DockerDiscovery,
    run: &mut lxcup_core::DockerDiscoveryRun,
) -> Result<Vec<DockerWorkloadDto>, ApiError> {
    for container in &discovered.containers {
        let workload = DockerWorkload {
            host_container_id: container_id,
            id: container.id.clone(),
            name: container.name.clone(),
            image: container.image.clone(),
            state: container.state.clone(),
            status: container.status.clone(),
            ports: container.ports.clone(),
            started_at: container.started_at.clone(),
            labels: container.labels.clone(),
            presence: lxcup_core::DockerWorkloadPresence::Present,
            change: lxcup_core::DockerWorkloadChange::New,
            management_state: DockerWorkloadManagementState::Discovered,
            discovered_at: chrono::Utc::now(),
        };
        if repositories
            .docker_workloads
            .upsert_discovered(&workload)
            .await
            .is_err()
        {
            return fail_inventory_persistence(
                state,
                run,
                "Docker-Inventar konnte nicht gespeichert werden",
            )
            .await;
        }
    }
    let seen_ids = discovered
        .containers
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    if repositories
        .docker_workloads
        .mark_missing_except(container_id, &seen_ids)
        .await
        .is_err()
    {
        return fail_inventory_persistence(
            state,
            run,
            "Docker-Inventar konnte nicht abgeglichen werden",
        )
        .await;
    }
    match repositories.docker_workloads.list(container_id).await {
        Ok(workloads) => Ok(workloads.into_iter().map(DockerWorkloadDto::from).collect()),
        Err(_) => {
            fail_inventory_persistence(state, run, "Docker-Inventar konnte nicht gelesen werden")
                .await
        }
    }
}

async fn fail_inventory_persistence<T>(
    state: &ApiState,
    run: &mut lxcup_core::DockerDiscoveryRun,
    message: &'static str,
) -> Result<T, ApiError> {
    finish_docker_discovery(state, run, 0, Some("docker_inventory_unavailable")).await?;
    Err(ApiError::dependency(
        "docker_inventory_unavailable",
        message,
    ))
}

async fn reconcile_memory_docker_inventory(
    state: &ApiState,
    container_id: ContainerId,
    discovered: lxcup_agent::DockerDiscovery,
) -> Vec<DockerWorkloadDto> {
    let mut store = state.store.write().await;
    let seen_ids = discovered
        .containers
        .iter()
        .map(|item| item.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for container in discovered.containers {
        let key = (container_id, container.id.clone());
        let previous = store.docker_workloads.get(&key).cloned();
        let management_state = previous
            .as_ref()
            .map(|item| item.management_state.clone())
            .unwrap_or_else(|| "discovered".to_owned());
        let change_state = docker_workload_change(previous.as_ref(), &container);
        let mut workload = DockerWorkloadDto::discovered(container_id, container);
        workload.management_state = management_state;
        workload.change_state = change_state.to_owned();
        store.docker_workloads.insert(key, workload);
    }
    mark_missing_docker_workloads(&mut store, container_id, &seen_ids);
    store
        .docker_workloads
        .values()
        .filter(|item| item.host_container_id == container_id)
        .cloned()
        .collect()
}

pub(super) fn docker_workload_change(
    previous: Option<&DockerWorkloadDto>,
    current: &lxcup_agent::DockerContainerInfo,
) -> &'static str {
    let Some(previous) = previous else {
        return "new";
    };
    if previous.presence == "missing"
        || previous.name != current.name
        || previous.image != current.image
        || previous.state != current.state
        || previous.status != current.status
        || previous.ports != current.ports
        || previous.started_at != current.started_at
        || previous.labels != current.labels
    {
        "changed"
    } else {
        "unchanged"
    }
}

fn mark_missing_docker_workloads(
    store: &mut crate::ApiStore,
    container_id: ContainerId,
    seen_ids: &std::collections::HashSet<String>,
) {
    for ((host_id, id), workload) in store.docker_workloads.iter_mut() {
        if *host_id == container_id && !seen_ids.contains(id) && workload.presence == "present" {
            workload.presence = "missing".to_owned();
            workload.change_state = "missing".to_owned();
        }
    }
}

async fn finish_docker_discovery(
    state: &ApiState,
    run: &mut lxcup_core::DockerDiscoveryRun,
    container_count: u32,
    error_code: Option<&str>,
) -> Result<lxcup_core::DockerDiscoveryRun, ApiError> {
    let status = if error_code.is_some() {
        lxcup_core::DockerDiscoveryStatus::Failed
    } else {
        lxcup_core::DockerDiscoveryStatus::Succeeded
    };
    if let Some(repositories) = state.repositories.as_ref() {
        *run = repositories
            .docker_discovery
            .finish(run.id, status, container_count, error_code)
            .await
            .map_err(|_| ApiError::storage())?;
    } else {
        run.status = status;
        run.finished_at = Some(chrono::Utc::now());
        run.container_count = container_count;
        run.error_code = error_code.map(str::to_owned);
        if let Some(stored) = state
            .store
            .write()
            .await
            .docker_discovery_runs
            .iter_mut()
            .find(|stored| stored.id == run.id)
        {
            *stored = run.clone();
        }
    }
    state.publish(ApiEvent::status(
        "docker_discovery",
        run.host_container_id.value().to_string(),
        if error_code.is_some() {
            "failed"
        } else {
            "succeeded"
        },
    ));
    if let Some(code) = error_code {
        state.publish(ApiEvent::Error {
            code: code.to_owned(),
            message: "Docker discovery failed".to_owned(),
            request_id: run.id.to_string(),
        });
    }
    Ok(run.clone())
}

pub(crate) async fn adopt_docker_container(
    State(state): State<ApiState>,
    Path((container_id, docker_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<DockerWorkloadDto>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    if let Some(repositories) = state.repositories.as_ref() {
        if !repositories
            .docker_workloads
            .adopt(container_id, &docker_id)
            .await
            .map_err(|_| {
                ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht aktualisiert werden",
                )
            })?
        {
            return Err(ApiError::not_found("Docker-Container zuerst entdecken"));
        }
        let item = repositories
            .docker_workloads
            .list(container_id)
            .await
            .map_err(|_| {
                ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht gelesen werden",
                )
            })?
            .into_iter()
            .find(|item| item.id == docker_id)
            .ok_or_else(|| ApiError::not_found("Docker-Container nicht im Inventar"))?;
        return Ok(Json(envelope(DockerWorkloadDto::from(item))));
    }
    let mut store = state.store.write().await;
    let workload = store
        .docker_workloads
        .get_mut(&(container_id, docker_id))
        .ok_or_else(|| ApiError::not_found("Docker-Container zuerst entdecken"))?;
    workload.management_state = "managed".to_owned();
    Ok(Json(envelope(workload.clone())))
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RemoveDockerWorkloadRequest {
    confirmed: bool,
}

pub(crate) async fn remove_docker_container(
    State(state): State<ApiState>,
    Path((container_id, docker_id)): Path<(String, String)>,
    JsonBody(request): JsonBody<RemoveDockerWorkloadRequest>,
) -> Result<StatusCode, ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Entfernen muss ausdrücklich bestätigt werden",
        ));
    }
    let container_id = parse_container_id(&container_id)?;
    if let Some(repositories) = state.repositories.as_ref() {
        if !repositories
            .docker_workloads
            .remove(container_id, &docker_id)
            .await
            .map_err(|_| {
                ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht aktualisiert werden",
                )
            })?
        {
            return Err(ApiError::not_found("Docker-Container nicht im Inventar"));
        }
        state.publish(ApiEvent::status(
            "docker",
            container_id.value().to_string(),
            "removed",
        ));
        return Ok(StatusCode::NO_CONTENT);
    }
    if state
        .store
        .write()
        .await
        .docker_workloads
        .remove(&(container_id, docker_id))
        .is_none()
    {
        return Err(ApiError::not_found("Docker-Container nicht im Inventar"));
    }
    state.publish(ApiEvent::status(
        "docker",
        container_id.value().to_string(),
        "removed",
    ));
    Ok(StatusCode::NO_CONTENT)
}
