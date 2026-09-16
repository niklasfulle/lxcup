//! HTTP API boundary for the lxcup web frontend and agents.

use std::{convert::Infallible, sync::Arc};

use axum::{
    Json, Router,
    extract::{Json as JsonBody, Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use lxcup_core::{
    Container, ContainerId, Execution, ExecutionId, Node, NodeId, PlanStatus, Scan, ScanId,
    UpdatePlan, UpdatePlanId,
};
use lxcup_planner::{DryRunChange, PlannerInput, UpdatePlanner};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    store: Arc<RwLock<ApiStore>>,
    events: broadcast::Sender<ApiEvent>,
}

impl ApiState {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            store: Arc::new(RwLock::new(ApiStore::default())),
            events,
        }
    }

    pub async fn replace_nodes(&self, nodes: Vec<Node>) {
        self.store.write().await.nodes = nodes;
    }

    pub async fn replace_containers(&self, containers: Vec<Container>) {
        self.store.write().await.containers = containers;
    }

    pub fn publish(&self, event: ApiEvent) {
        let _ = self.events.send(event);
    }
}

impl Default for ApiState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct ApiStore {
    nodes: Vec<Node>,
    containers: Vec<Container>,
    scans: Vec<Scan>,
    plans: Vec<UpdatePlan>,
    executions: Vec<Execution>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route(
            "/api/v1/nodes/{node_id}/containers",
            get(list_node_containers),
        )
        .route("/api/v1/containers", get(list_containers))
        .route(
            "/api/v1/containers/{container_id}/scans",
            get(list_scans).post(start_scan),
        )
        .route(
            "/api/v1/containers/{container_id}/plans",
            get(list_container_plans).post(create_plan),
        )
        .route("/api/v1/plans/{plan_id}", get(get_plan))
        .route("/api/v1/plans/{plan_id}/confirm", post(confirm_plan))
        .route(
            "/api/v1/executions/{execution_id}/abort",
            post(abort_execution),
        )
        .route("/api/v1/executions/{execution_id}", get(get_execution))
        .route("/api/v1/openapi.json", get(openapi_document))
        .route("/api/v1/events", get(stream_events))
        .with_state(state)
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorInfo,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiErrorInfo {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl ApiError {
    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message,
        }
    }

    fn not_found(resource: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: resource,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiErrorInfo {
                code: self.code,
                message: self.message,
            },
            request_id: Uuid::new_v4(),
        };
        (self.status, Json(body)).into_response()
    }
}

async fn list_nodes(State(state): State<ApiState>) -> Json<ApiEnvelope<Vec<NodeDto>>> {
    let nodes = state
        .store
        .read()
        .await
        .nodes
        .iter()
        .map(NodeDto::from)
        .collect();
    Json(envelope(nodes))
}

async fn list_node_containers(
    State(state): State<ApiState>,
    Path(node_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<ContainerDto>>>, ApiError> {
    let node_id = parse_node_id(&node_id)?;
    let containers = state
        .store
        .read()
        .await
        .containers
        .iter()
        .filter(|container| container.node_id == node_id)
        .map(ContainerDto::from)
        .collect();
    Ok(Json(envelope(containers)))
}

async fn list_containers(State(state): State<ApiState>) -> Json<ApiEnvelope<Vec<ContainerDto>>> {
    let containers = state
        .store
        .read()
        .await
        .containers
        .iter()
        .map(ContainerDto::from)
        .collect();
    Json(envelope(containers))
}

async fn list_scans(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<ScanDto>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let scans = state
        .store
        .read()
        .await
        .scans
        .iter()
        .filter(|scan| scan.container_id == container_id)
        .map(ScanDto::from)
        .collect();
    Ok(Json(envelope(scans)))
}

async fn start_scan(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<(StatusCode, Json<ApiEnvelope<ScanDto>>), ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let mut store = state.store.write().await;
    if !store
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }
    let scan = Scan::new(container_id);
    let dto = ScanDto::from(&scan);
    store.scans.push(scan);
    drop(store);
    state.publish(ApiEvent::status(
        "scan",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreatePlanRequest {
    pub requested_packages: Vec<String>,
    #[serde(default)]
    pub dry_run_changes: Vec<DryRunChangeRequest>,
    #[serde(default)]
    pub distribution_upgrade: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DryRunChangeRequest {
    pub package: String,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub kind: lxcup_core::PackageChangeKind,
    #[serde(default)]
    pub held: bool,
    #[serde(default = "default_authenticated")]
    pub authenticated: bool,
}

fn default_authenticated() -> bool {
    true
}

async fn list_container_plans(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<PlanDto>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let plans = state
        .store
        .read()
        .await
        .plans
        .iter()
        .filter(|plan| plan.container_id == container_id)
        .map(PlanDto::from)
        .collect();
    Ok(Json(envelope(plans)))
}

async fn create_plan(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<CreatePlanRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<PlanDto>>), ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let dry_run_changes = request
        .dry_run_changes
        .into_iter()
        .map(|change| DryRunChange {
            package: change.package,
            from_version: change.from_version,
            to_version: change.to_version,
            kind: change.kind,
            held: change.held,
            authenticated: change.authenticated,
        })
        .collect();
    let plan = UpdatePlanner
        .build(PlannerInput {
            container_id,
            requested_packages: request.requested_packages,
            dry_run_changes,
            distribution_upgrade: request.distribution_upgrade,
        })
        .map_err(|_| ApiError::bad_request("invalid_plan", "plan input is not valid"))?;
    let dto = PlanDto::from(&plan);
    state.store.write().await.plans.push(plan);
    state.publish(ApiEvent::status(
        "plan",
        dto.id.as_uuid().to_string(),
        dto.status.as_str(),
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

async fn get_plan(
    State(state): State<ApiState>,
    Path(plan_id): Path<String>,
) -> Result<Json<ApiEnvelope<PlanDto>>, ApiError> {
    let plan_id = parse_uuid(&plan_id, "plan id")?;
    let store = state.store.read().await;
    let plan = store
        .plans
        .iter()
        .find(|plan| plan.id.as_uuid() == plan_id)
        .ok_or_else(|| ApiError::not_found("plan not found"))?;
    Ok(Json(envelope(PlanDto::from(plan))))
}

async fn confirm_plan(
    State(state): State<ApiState>,
    Path(plan_id): Path<String>,
) -> Result<(StatusCode, Json<ApiEnvelope<ExecutionDto>>), ApiError> {
    let plan_id = parse_uuid(&plan_id, "plan id")?;
    let mut store = state.store.write().await;
    let plan = store
        .plans
        .iter_mut()
        .find(|plan| plan.id.as_uuid() == plan_id)
        .ok_or_else(|| ApiError::not_found("plan not found"))?;
    plan.transition_to(PlanStatus::Confirmed).map_err(|_| {
        ApiError::bad_request("plan_not_ready", "plan is not ready for confirmation")
    })?;
    let execution = Execution::new(plan.id);
    let dto = ExecutionDto::from(&execution);
    store.executions.push(execution);
    drop(store);
    state.publish(ApiEvent::task(
        dto.id.as_uuid().to_string(),
        "execution_queued",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

async fn abort_execution(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
) -> Result<Json<ApiEnvelope<ExecutionDto>>, ApiError> {
    let execution_id = parse_uuid(&execution_id, "execution id")?;
    let mut store = state.store.write().await;
    let execution = store
        .executions
        .iter_mut()
        .find(|execution| execution.id.as_uuid() == execution_id)
        .ok_or_else(|| ApiError::not_found("execution not found"))?;
    execution
        .transition_to(lxcup_core::ExecutionStatus::Aborted, chrono::Utc::now())
        .map_err(|_| {
            ApiError::bad_request("execution_not_abortable", "execution cannot be aborted")
        })?;
    let dto = ExecutionDto::from(&*execution);
    drop(store);
    state.publish(ApiEvent::task(
        dto.id.as_uuid().to_string(),
        "execution_aborted",
    ));
    Ok(Json(envelope(dto)))
}

async fn get_execution(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
) -> Result<Json<ApiEnvelope<ExecutionDto>>, ApiError> {
    let execution_id = parse_uuid(&execution_id, "execution id")?;
    let store = state.store.read().await;
    let execution = store
        .executions
        .iter()
        .find(|execution| execution.id.as_uuid() == execution_id)
        .ok_or_else(|| ApiError::not_found("execution not found"))?;
    Ok(Json(envelope(ExecutionDto::from(execution))))
}

/// Versioned machine-readable contract consumed by the frontend and API clients.
pub const OPENAPI_CONTRACT: &str = r#"{
  "openapi": "3.1.0",
  "info": {"title": "lxcup API", "version": "v1"},
  "paths": {
    "/api/v1/nodes": {"get": {"responses": {"200": {"description": "Nodes"}}}},
    "/api/v1/containers": {"get": {"responses": {"200": {"description": "Containers"}}}},
    "/api/v1/containers/{container_id}/scans": {"get": {}, "post": {}},
    "/api/v1/containers/{container_id}/plans": {"get": {}, "post": {}},
    "/api/v1/plans/{plan_id}": {"get": {}},
    "/api/v1/plans/{plan_id}/confirm": {"post": {}},
    "/api/v1/executions/{execution_id}": {"get": {}},
    "/api/v1/executions/{execution_id}/abort": {"post": {}},
    "/api/v1/events": {"get": {"description": "Typed task, log and status SSE"}}
  }
}"#;

async fn openapi_document() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        OPENAPI_CONTRACT,
    )
}

async fn stream_events(
    State(state): State<ApiState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(event) => Some(Ok(Event::default()
            .event(event.kind())
            .json_data(&event)
            .unwrap_or_else(|_| Event::default()))),
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

fn envelope<T>(data: T) -> ApiEnvelope<T> {
    ApiEnvelope {
        data,
        request_id: Uuid::new_v4(),
    }
}

fn parse_uuid(value: &str, label: &'static str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::bad_request("invalid_id", label))
}

fn parse_node_id(value: &str) -> Result<NodeId, ApiError> {
    Ok(NodeId::from_uuid(parse_uuid(value, "node id")?))
}

fn parse_container_id(value: &str) -> Result<ContainerId, ApiError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(ContainerId::new)
        .ok_or_else(|| ApiError::bad_request("invalid_id", "container id"))
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeDto {
    pub id: NodeId,
    pub name: String,
    pub address: String,
    pub status: String,
    pub proxmox_version: Option<String>,
    pub capabilities: Vec<String>,
    pub last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&Node> for NodeDto {
    fn from(node: &Node) -> Self {
        Self {
            id: node.id,
            name: node.name.clone(),
            address: node.address.clone(),
            status: format!("{:?}", node.status).to_ascii_lowercase(),
            proxmox_version: node.proxmox_version.clone(),
            capabilities: node.capabilities.clone(),
            last_checked_at: node.last_checked_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerDto {
    pub id: ContainerId,
    pub node_id: NodeId,
    pub name: String,
    pub operating_system: String,
    pub status: String,
    pub management_state: String,
    pub discovered_at: chrono::DateTime<chrono::Utc>,
}

impl From<&Container> for ContainerDto {
    fn from(container: &Container) -> Self {
        Self {
            id: container.id,
            node_id: container.node_id,
            name: container.name.clone(),
            operating_system: format!("{:?}", container.operating_system).to_ascii_lowercase(),
            status: format!("{:?}", container.status).to_ascii_lowercase(),
            management_state: format!("{:?}", container.management_state).to_ascii_lowercase(),
            discovered_at: container.discovered_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ScanDto {
    pub id: ScanId,
    pub container_id: ContainerId,
    pub status: String,
    pub update_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&Scan> for ScanDto {
    fn from(scan: &Scan) -> Self {
        Self {
            id: scan.id,
            container_id: scan.container_id,
            status: format!("{:?}", scan.status).to_ascii_lowercase(),
            update_count: scan.updates.len(),
            created_at: scan.created_at,
            completed_at: scan.completed_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanDto {
    pub id: UpdatePlanId,
    pub container_id: ContainerId,
    pub status: String,
    pub requested_packages: Vec<String>,
    pub change_count: usize,
    pub plan_hash: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<&UpdatePlan> for PlanDto {
    fn from(plan: &UpdatePlan) -> Self {
        Self {
            id: plan.id,
            container_id: plan.container_id,
            status: format!("{:?}", plan.status).to_ascii_lowercase(),
            requested_packages: plan
                .requested_packages
                .iter()
                .map(|package| package.as_str().to_owned())
                .collect(),
            change_count: plan.resolved_changes.len(),
            plan_hash: plan.plan_hash.clone(),
            created_at: plan.created_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionDto {
    pub id: ExecutionId,
    pub plan_id: UpdatePlanId,
    pub status: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&Execution> for ExecutionDto {
    fn from(execution: &Execution) -> Self {
        Self {
            id: execution.id,
            plan_id: execution.plan_id,
            status: format!("{:?}", execution.status).to_ascii_lowercase(),
            started_at: execution.started_at,
            finished_at: execution.finished_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum ApiEvent {
    Task {
        task_id: String,
        state: String,
    },
    Log {
        source: String,
        message: String,
    },
    Status {
        resource: String,
        resource_id: String,
        state: String,
    },
}

impl ApiEvent {
    fn task(task_id: String, state: &'static str) -> Self {
        Self::Task {
            task_id,
            state: state.to_owned(),
        }
    }
    fn status(resource: &'static str, resource_id: String, state: impl Into<String>) -> Self {
        Self::Status {
            resource: resource.to_owned(),
            resource_id,
            state: state.into(),
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Task { .. } => "task",
            Self::Log { .. } => "log",
            Self::Status { .. } => "status",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn nodes_endpoint_returns_versioned_envelope() {
        let state = ApiState::new();
        state
            .replace_nodes(vec![Node::new("pve01", "https://pve01:8006").unwrap()])
            .await;
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!(parse_container_id("0").is_err());
        assert!(parse_uuid("not-an-id", "plan id").is_err());
    }
}
