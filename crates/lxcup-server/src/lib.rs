//! HTTP API boundary for the lxcup web frontend and agents.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    extract::{Json as JsonBody, Path, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use lxcup_agent::{
    AgentAction, AgentClient, AgentClientConfig, AgentCommandRequest, AgentHealth, AgentMetrics,
};
use lxcup_core::{
    Container, ContainerId, Enrollment, EnrollmentId, EnrollmentState, Execution, ExecutionId,
    Node, NodeId, PlanStatus, Scan, ScanId, UpdatePlan, UpdatePlanId,
};
use lxcup_execution::{ExecutionCoordinator, ExecutionRequest, ExecutionStart};
use lxcup_persistence::Repositories;
use lxcup_planner::{DryRunChange, PlannerInput, UpdatePlanner};
use lxcup_safety::{HealthCheckResult, RebootRequirement, SnapshotState};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    store: Arc<RwLock<ApiStore>>,
    execution: Arc<RwLock<ExecutionCoordinator>>,
    events: broadcast::Sender<ApiEvent>,
    agents: Arc<RwLock<HashMap<ContainerId, RegisteredAgent>>>,
    metrics: Arc<ApiMetrics>,
    auth: AuthConfig,
    repositories: Option<Repositories>,
}

impl ApiState {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            store: Arc::new(RwLock::new(ApiStore::default())),
            execution: Arc::new(RwLock::new(ExecutionCoordinator::default())),
            events,
            agents: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(ApiMetrics::default()),
            auth: AuthConfig::from_env(),
            repositories: None,
        }
    }

    pub fn with_repositories(mut self, repositories: Repositories) -> Self {
        self.repositories = Some(repositories);
        self
    }

    pub fn with_auth_config(mut self, auth: AuthConfig) -> Self {
        self.auth = auth;
        self
    }

    pub async fn replace_nodes(&self, nodes: Vec<Node>) {
        self.store.write().await.nodes = nodes;
    }

    pub async fn replace_containers(&self, containers: Vec<Container>) {
        self.store.write().await.containers = containers;
    }

    pub fn publish(&self, event: ApiEvent) {
        self.metrics
            .events_published
            .fetch_add(1, Ordering::Relaxed);
        let _ = self.events.send(event);
    }

    pub async fn record_safety(&self, execution_id: ExecutionId, report: SafetyDto) {
        self.store.write().await.safety.insert(execution_id, report);
        self.publish(ApiEvent::status(
            "safety",
            execution_id.as_uuid().to_string(),
            "updated",
        ));
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
    enrollments: Vec<Enrollment>,
    enrollment_keys: HashMap<String, EnrollmentId>,
    scans: Vec<Scan>,
    plans: Vec<UpdatePlan>,
    executions: Vec<Execution>,
    safety: HashMap<ExecutionId, SafetyDto>,
    results: HashMap<ExecutionId, ExecutionResultDto>,
}

#[derive(Clone)]
struct RegisteredAgent {
    client: AgentClient,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateEnrollmentRequest {
    pub container_id: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnrollmentDto {
    pub id: EnrollmentId,
    pub container_id: ContainerId,
    pub state: EnrollmentState,
    pub failure_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&Enrollment> for EnrollmentDto {
    fn from(enrollment: &Enrollment) -> Self {
        Self {
            id: enrollment.id,
            container_id: enrollment.container_id,
            state: enrollment.state,
            failure_reason: enrollment.failure_reason.clone(),
            created_at: enrollment.created_at,
            updated_at: enrollment.updated_at,
        }
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health/live", get(live_health))
        .route("/health/ready", get(ready_health))
        .route("/metrics", get(metrics))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/enrollments", post(create_enrollment))
        .route("/api/v1/enrollments/{enrollment_id}", get(get_enrollment))
        .route(
            "/api/v1/nodes/{node_id}/containers",
            get(list_node_containers),
        )
        .route("/api/v1/containers", get(list_containers))
        .route(
            "/api/v1/containers/{container_id}/scans",
            get(list_scans).post(start_scan),
        )
        .route("/api/v1/scans/{scan_id}/run", post(run_scan))
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
        .route("/api/v1/executions/{execution_id}/run", post(run_execution))
        .route(
            "/api/v1/executions/{execution_id}/reconcile",
            post(reconcile_execution),
        )
        .route("/api/v1/executions/{execution_id}", get(get_execution))
        .route(
            "/api/v1/executions/{execution_id}/result",
            get(get_execution_result),
        )
        .route("/api/v1/executions/{execution_id}/safety", get(get_safety))
        .route(
            "/api/v1/containers/{container_id}/agent",
            post(register_agent),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/health",
            get(get_agent_health),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/metrics",
            get(get_agent_metrics),
        )
        .route("/api/v1/openapi.json", get(openapi_document))
        .route("/api/v1/events", get(stream_events))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, request_middleware))
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

    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
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

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "authentication is required",
        }
    }

    fn dependency(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code,
            message,
        }
    }

    fn storage() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "the operation could not be persisted",
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

async fn create_enrollment(
    State(state): State<ApiState>,
    JsonBody(request): JsonBody<CreateEnrollmentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<EnrollmentDto>>), ApiError> {
    let container_id = ContainerId::new(request.container_id);
    if request.container_id == 0 {
        return Err(ApiError::bad_request(
            "invalid_container_id",
            "container id must be greater than zero",
        ));
    }

    let mut store = state.store.write().await;
    if !store
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }

    if let Some(existing_id) = store.enrollment_keys.get(&request.idempotency_key).copied() {
        let existing = store
            .enrollments
            .iter()
            .find(|enrollment| enrollment.id == existing_id)
            .expect("enrollment idempotency index must point to an enrollment");
        if existing.container_id != container_id {
            return Err(ApiError::conflict(
                "idempotency_key_reused",
                "idempotency key is already used for another container",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(envelope(EnrollmentDto::from(existing))),
        ));
    }

    if store
        .enrollments
        .iter()
        .any(|enrollment| enrollment.container_id == container_id && enrollment.state.is_active())
    {
        return Err(ApiError::conflict(
            "enrollment_in_progress",
            "an enrollment is already in progress for this container",
        ));
    }

    let enrollment =
        Enrollment::new(container_id, request.idempotency_key.clone()).map_err(|_| {
            ApiError::bad_request("invalid_idempotency_key", "idempotency key is invalid")
        })?;
    let dto = EnrollmentDto::from(&enrollment);
    store
        .enrollment_keys
        .insert(request.idempotency_key, enrollment.id);
    store.enrollments.push(enrollment);
    drop(store);

    state.publish(ApiEvent::status(
        "enrollment",
        dto.id.as_uuid().to_string(),
        "requested",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

async fn get_enrollment(
    State(state): State<ApiState>,
    Path(enrollment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnrollmentDto>>, ApiError> {
    let enrollment_id = EnrollmentId::from_uuid(parse_uuid(&enrollment_id, "enrollment id")?);
    let store = state.store.read().await;
    let enrollment = store
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .ok_or_else(|| ApiError::not_found("enrollment not found"))?;
    Ok(Json(envelope(EnrollmentDto::from(enrollment))))
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
    store.scans.push(scan.clone());
    let repositories = state.repositories.clone();
    drop(store);
    if let Some(repositories) = repositories {
        repositories
            .scans
            .save(&scan)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(ApiEvent::status(
        "scan",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    viewer_token: Option<Arc<str>>,
    operator_token: Option<Arc<str>>,
    admin_token: Option<Arc<str>>,
    required: bool,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let viewer_token = std::env::var("LXCUP_AUTH_VIEWER_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let operator_token = std::env::var("LXCUP_AUTH_OPERATOR_TOKEN")
            .ok()
            .or_else(|| std::env::var("LXCUP_AUTH_TOKEN").ok())
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let admin_token = std::env::var("LXCUP_AUTH_ADMIN_TOKEN")
            .ok()
            .or_else(|| std::env::var("LXCUP_AUTH_TOKEN").ok())
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let required = std::env::var("LXCUP_AUTH_REQUIRED")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(operator_token.is_some() || viewer_token.is_some());
        Self {
            viewer_token,
            operator_token,
            admin_token,
            required,
        }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn with_tokens(
        mut self,
        viewer: Option<String>,
        operator: Option<String>,
        admin: Option<String>,
    ) -> Self {
        self.viewer_token = viewer.map(Arc::<str>::from);
        self.operator_token = operator.map(Arc::<str>::from);
        self.admin_token = admin.map(Arc::<str>::from);
        self
    }

    fn allows(&self, method: &Method, headers: &HeaderMap) -> bool {
        if !self.required {
            return true;
        }
        let Some(token) = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        if self.admin_token.as_deref() == Some(token)
            || self.operator_token.as_deref() == Some(token)
        {
            return true;
        }
        method == Method::GET && self.viewer_token.as_deref() == Some(token)
    }
}

struct ApiMetrics {
    started_at: Option<Instant>,
    requests_total: AtomicU64,
    requests_failed: AtomicU64,
    events_published: AtomicU64,
}

impl Default for ApiMetrics {
    fn default() -> Self {
        Self {
            started_at: Some(Instant::now()),
            requests_total: AtomicU64::default(),
            requests_failed: AtomicU64::default(),
            events_published: AtomicU64::default(),
        }
    }
}

impl ApiMetrics {
    fn started_at(&self) -> Instant {
        self.started_at.unwrap_or_else(Instant::now)
    }

    fn render(&self) -> String {
        format!(
            "# TYPE lxcup_http_requests_total counter\nlxcup_http_requests_total {}\n# TYPE lxcup_http_requests_failed_total counter\nlxcup_http_requests_failed_total {}\n# TYPE lxcup_events_published_total counter\nlxcup_events_published_total {}\nlxcup_uptime_seconds {}\n",
            self.requests_total.load(Ordering::Relaxed),
            self.requests_failed.load(Ordering::Relaxed),
            self.events_published.load(Ordering::Relaxed),
            self.started_at().elapsed().as_secs()
        )
    }
}

async fn request_middleware(
    State(state): State<ApiState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let public = path == "/health/live" || path == "/health/ready" || path == "/metrics";
    if !public && !state.auth.allows(request.method(), request.headers()) {
        return ApiError::unauthorized().into_response();
    }
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let response = next.run(request).await;
    if response.status().is_client_error() || response.status().is_server_error() {
        state
            .metrics
            .requests_failed
            .fetch_add(1, Ordering::Relaxed);
    }
    response
}

async fn live_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn ready_health(State(state): State<ApiState>) -> impl IntoResponse {
    let ready = state
        .store
        .read()
        .await
        .nodes
        .iter()
        .all(|node| node.status != lxcup_core::NodeStatus::Disconnected);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({ "status": if ready { "ready" } else { "degraded" } })),
    )
}

async fn metrics(State(state): State<ApiState>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        state.metrics.render(),
    )
}

async fn run_scan(
    State(state): State<ApiState>,
    Path(scan_id): Path<String>,
) -> Result<Json<ApiEnvelope<ScanDto>>, ApiError> {
    let scan_id = parse_uuid(&scan_id, "scan id")?;
    let scan_id = ScanId::from_uuid(scan_id);
    let (container_id, agent) = {
        let mut store = state.store.write().await;
        let scan = store
            .scans
            .iter_mut()
            .find(|scan| scan.id == scan_id)
            .ok_or_else(|| ApiError::not_found("scan not found"))?;
        scan.transition_to(lxcup_core::ScanStatus::Running, chrono::Utc::now())
            .map_err(|_| ApiError::bad_request("scan_not_runnable", "scan is not runnable"))?;
        let container_id = scan.container_id;
        let agent = state
            .agents
            .read()
            .await
            .get(&container_id)
            .cloned()
            .ok_or_else(|| {
                ApiError::dependency(
                    "agent_unavailable",
                    "no agent is registered for this container",
                )
            })?;
        (container_id, agent)
    };
    let response = agent
        .client
        .command(&AgentCommandRequest {
            action: AgentAction::Scan,
            packages: Vec::new(),
            idempotency_key: scan_id.as_uuid().to_string(),
        })
        .await
        .map_err(|_| {
            ApiError::dependency("agent_request_failed", "the agent scan request failed")
        })?;
    let mut store = state.store.write().await;
    let scan = store
        .scans
        .iter_mut()
        .find(|scan| scan.id == scan_id)
        .ok_or_else(|| ApiError::not_found("scan not found"))?;
    if response.success {
        match lxcup_apt::build_scan(
            container_id,
            lxcup_apt::AptExecutionResult {
                stdout: response.stdout,
                stderr: response.stderr,
                exit_code: Some(response.exit_code),
            },
        ) {
            Ok(mut result) => {
                result.scan.id = scan_id;
                *scan = result.scan;
            }
            Err(_) => {
                scan.transition_to(lxcup_core::ScanStatus::Failed, chrono::Utc::now())
                    .map_err(|_| {
                        ApiError::bad_request("scan_failed", "the agent returned invalid scan data")
                    })?;
            }
        }
    } else {
        scan.transition_to(lxcup_core::ScanStatus::Failed, chrono::Utc::now())
            .map_err(|_| ApiError::bad_request("scan_failed", "the agent scan failed"))?;
    }
    let dto = ScanDto::from(&*scan);
    drop(store);
    state.publish(ApiEvent::status(
        "scan",
        scan_id.as_uuid().to_string(),
        dto.status.clone(),
    ));
    Ok(Json(envelope(dto)))
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
    let repositories = state.repositories.clone();
    state.store.write().await.plans.push(plan.clone());
    if let Some(repositories) = repositories {
        repositories
            .plans
            .save(&plan)
            .await
            .map_err(|_| ApiError::storage())?;
    }
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
    headers: HeaderMap,
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
    let planner_input = PlannerInput {
        container_id: plan.container_id,
        requested_packages: plan
            .requested_packages
            .iter()
            .map(|package| package.as_str().to_owned())
            .collect(),
        dry_run_changes: plan
            .resolved_changes
            .iter()
            .map(|change| DryRunChange {
                package: change.package.as_str().to_owned(),
                from_version: change
                    .from_version
                    .as_ref()
                    .map(|version| version.as_str().to_owned()),
                to_version: change
                    .to_version
                    .as_ref()
                    .map(|version| version.as_str().to_owned()),
                kind: change.kind,
                held: false,
                authenticated: true,
            })
            .collect(),
        distribution_upgrade: false,
    };
    let actor = headers
        .get("x-actor")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("api")
        .to_owned();
    let default_idempotency_key = plan_id.to_string();
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(&default_idempotency_key)
        .to_owned();
    let start = state
        .execution
        .write()
        .await
        .start(ExecutionRequest {
            plan: plan.clone(),
            current_plan_input: planner_input,
            actor,
            idempotency_key,
        })
        .map_err(|_| ApiError::bad_request("execution_rejected", "execution was rejected"))?;
    let created = matches!(&start, ExecutionStart::Created(_));
    let execution = match start {
        ExecutionStart::Created(execution) => {
            store.executions.push(execution.clone());
            execution
        }
        ExecutionStart::Duplicate(execution_id) => store
            .executions
            .iter()
            .find(|execution| execution.id == execution_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("execution not found"))?,
    };
    let dto = ExecutionDto::from(&execution);
    let repositories = state.repositories.clone();
    drop(store);
    if let Some(repositories) = repositories {
        if created {
            repositories
                .executions
                .save(&execution)
                .await
                .map_err(|_| ApiError::storage())?;
        }
    }
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
    let execution_id = lxcup_core::ExecutionId::from_uuid(execution_id);
    let aborted = state
        .execution
        .write()
        .await
        .abort(execution_id)
        .map_err(|_| {
            ApiError::bad_request("execution_not_abortable", "execution cannot be aborted")
        })?;
    let dto = ExecutionDto::from(&aborted);
    let mut store = state.store.write().await;
    if let Some(execution) = store
        .executions
        .iter_mut()
        .find(|execution| execution.id == execution_id)
    {
        *execution = aborted;
    }
    drop(store);
    state.publish(ApiEvent::task(
        dto.id.as_uuid().to_string(),
        "execution_aborted",
    ));
    Ok(Json(envelope(dto)))
}

async fn run_execution(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
) -> Result<Json<ApiEnvelope<ExecutionDto>>, ApiError> {
    let execution_id =
        lxcup_core::ExecutionId::from_uuid(parse_uuid(&execution_id, "execution id")?);
    let plan = state
        .execution
        .read()
        .await
        .plan(execution_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("execution not found"))?;
    let agent = state
        .agents
        .read()
        .await
        .get(&plan.container_id)
        .cloned()
        .ok_or_else(|| {
            ApiError::dependency(
                "agent_unavailable",
                "no agent is registered for this container",
            )
        })?;
    state
        .execution
        .write()
        .await
        .begin(execution_id)
        .map_err(|_| {
            ApiError::bad_request("execution_not_runnable", "execution cannot be started")
        })?;
    let response = agent
        .client
        .command(&AgentCommandRequest {
            action: AgentAction::Apply,
            packages: plan
                .requested_packages
                .iter()
                .map(|package| package.as_str().to_owned())
                .collect(),
            idempotency_key: execution_id.as_uuid().to_string(),
        })
        .await;
    let result = match response {
        Ok(response) => lxcup_execution::ExecutionResult {
            exit_code: response.exit_code,
            stdout: response.stdout,
            stderr: response.stderr,
        },
        Err(error) => lxcup_execution::ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    };
    let execution = state
        .execution
        .write()
        .await
        .finish(execution_id, result.clone())
        .map_err(|_| {
            ApiError::bad_request("execution_failed", "execution could not be finalized")
        })?;
    let dto = ExecutionDto::from(&execution);
    let mut store = state.store.write().await;
    store.executions.retain(|item| item.id != execution_id);
    store.executions.push(execution.clone());
    store
        .results
        .insert(execution_id, ExecutionResultDto::from_result(&result));
    let repositories = state.repositories.clone();
    drop(store);
    if let Some(repositories) = repositories {
        repositories
            .executions
            .update(&execution)
            .await
            .map_err(|_| ApiError::storage())?;
        repositories
            .executions
            .save_result(&lxcup_persistence::ExecutionResultRecord {
                execution_id,
                exit_code: result.exit_code,
                stdout: truncate(&result.stdout),
                stderr: truncate(&result.stderr),
                created_at: chrono::Utc::now(),
            })
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(ApiEvent::task(
        execution_id.as_uuid().to_string(),
        dto.status.clone(),
    ));
    Ok(Json(envelope(dto)))
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReconcileRequest {
    pub observed: lxcup_execution::ObservedExecutionState,
}

async fn reconcile_execution(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
    JsonBody(request): JsonBody<ReconcileRequest>,
) -> Result<Json<ApiEnvelope<ExecutionDto>>, ApiError> {
    let execution_id =
        lxcup_core::ExecutionId::from_uuid(parse_uuid(&execution_id, "execution id")?);
    let (execution, action) = state
        .execution
        .write()
        .await
        .reconcile(execution_id, request.observed)
        .map_err(|_| {
            ApiError::bad_request(
                "reconciliation_failed",
                "execution state cannot be reconciled",
            )
        })?;
    let dto = ExecutionDto::from(&execution);
    let mut store = state.store.write().await;
    store.executions.retain(|item| item.id != execution_id);
    store.executions.push(execution);
    drop(store);
    state.publish(ApiEvent::task(
        execution_id.as_uuid().to_string(),
        format!("reconciled_{action:?}").to_ascii_lowercase(),
    ));
    Ok(Json(envelope(dto)))
}

async fn get_execution_result(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
) -> Result<Json<ApiEnvelope<ExecutionResultDto>>, ApiError> {
    let execution_id =
        lxcup_core::ExecutionId::from_uuid(parse_uuid(&execution_id, "execution id")?);
    let result = state
        .store
        .read()
        .await
        .results
        .get(&execution_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("execution result not found"))?;
    Ok(Json(envelope(result)))
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegisterAgentRequest {
    pub endpoint: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentRegistrationDto {
    pub endpoint: String,
    pub info: AgentHealth,
}

async fn register_agent(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<RegisterAgentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<AgentRegistrationDto>>), ApiError> {
    let container_id = parse_container_id(&container_id)?;
    if !state
        .store
        .read()
        .await
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }
    let config = AgentClientConfig::new(request.endpoint.clone(), request.token).map_err(|_| {
        ApiError::bad_request("invalid_agent_config", "agent endpoint or token is invalid")
    })?;
    let client = AgentClient::new(config).map_err(|_| {
        ApiError::dependency(
            "agent_client_error",
            "the agent client could not be created",
        )
    })?;
    let info = client
        .health()
        .await
        .map_err(|_| ApiError::dependency("agent_unavailable", "the agent healthcheck failed"))?;
    let dto = AgentRegistrationDto {
        endpoint: request.endpoint.clone(),
        info: info.clone(),
    };
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });
    state.publish(ApiEvent::status(
        "agent",
        container_id.value().to_string(),
        "registered",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

async fn get_agent_health(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<AgentHealth>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let agent = state
        .agents
        .read()
        .await
        .get(&container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("agent not registered"))?;
    let health =
        agent.client.health().await.map_err(|_| {
            ApiError::dependency("agent_unavailable", "the agent healthcheck failed")
        })?;
    Ok(Json(envelope(health)))
}

async fn get_agent_metrics(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<AgentMetrics>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let agent = state
        .agents
        .read()
        .await
        .get(&container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("agent not registered"))?;
    let metrics = agent.client.metrics().await.map_err(|_| {
        ApiError::dependency("agent_unavailable", "the agent metrics endpoint failed")
    })?;
    Ok(Json(envelope(metrics)))
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

#[derive(Clone, Debug, Serialize)]
pub struct SafetyDto {
    pub snapshot: SnapshotState,
    pub healthchecks: Vec<HealthCheckResult>,
    pub reboot: RebootRequirement,
    pub automatic_rollback: bool,
}

async fn get_safety(
    State(state): State<ApiState>,
    Path(execution_id): Path<String>,
) -> Result<Json<ApiEnvelope<SafetyDto>>, ApiError> {
    let execution_id = parse_uuid(&execution_id, "execution id")?;
    let store = state.store.read().await;
    let report = store
        .safety
        .get(&ExecutionId::from_uuid(execution_id))
        .cloned()
        .ok_or_else(|| ApiError::not_found("safety report not found"))?;
    Ok(Json(envelope(report)))
}

/// Versioned machine-readable contract consumed by the frontend and API clients.
pub const OPENAPI_CONTRACT: &str = r#"{
  "openapi": "3.1.0",
  "info": {"title": "lxcup API", "version": "v1"},
  "components": {"securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}}},
  "paths": {
    "/health/live": {"get": {}},
    "/health/ready": {"get": {}},
    "/metrics": {"get": {}},
    "/api/v1/nodes": {"get": {"responses": {"200": {"description": "Nodes"}}}},
    "/api/v1/enrollments": {"post": {"responses": {"202": {"description": "Enrollment accepted"}}}},
    "/api/v1/enrollments/{enrollment_id}": {"get": {"responses": {"200": {"description": "Enrollment status"}}}},
    "/api/v1/containers": {"get": {"responses": {"200": {"description": "Containers"}}}},
    "/api/v1/containers/{container_id}/scans": {"get": {}, "post": {}},
    "/api/v1/scans/{scan_id}/run": {"post": {}},
    "/api/v1/containers/{container_id}/plans": {"get": {}, "post": {}},
    "/api/v1/plans/{plan_id}": {"get": {}},
    "/api/v1/plans/{plan_id}/confirm": {"post": {}},
    "/api/v1/executions/{execution_id}": {"get": {}},
    "/api/v1/executions/{execution_id}/abort": {"post": {}},
    "/api/v1/executions/{execution_id}/run": {"post": {}},
    "/api/v1/executions/{execution_id}/reconcile": {"post": {}},
    "/api/v1/executions/{execution_id}/result": {"get": {}},
    "/api/v1/executions/{execution_id}/safety": {"get": {}},
    "/api/v1/containers/{container_id}/agent": {"post": {}},
    "/api/v1/containers/{container_id}/agent/health": {"get": {}},
    "/api/v1/containers/{container_id}/agent/metrics": {"get": {}},
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

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionResultDto {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecutionResultDto {
    fn from_result(result: &lxcup_execution::ExecutionResult) -> Self {
        Self {
            exit_code: result.exit_code,
            stdout: truncate(&result.stdout),
            stderr: truncate(&result.stderr),
        }
    }
}

fn truncate(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .chars()
        .take(4_000)
        .collect()
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
    Error {
        code: String,
        message: String,
        request_id: String,
    },
}

impl ApiEvent {
    fn task(task_id: String, state: impl Into<String>) -> Self {
        Self::Task {
            task_id,
            state: state.into(),
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
            Self::Error { .. } => "error",
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

    #[tokio::test]
    async fn enrollment_endpoint_is_idempotent_and_queryable() {
        let state = ApiState::new();
        let container = Container::new(
            ContainerId::new(101),
            NodeId::new(),
            "test-lxc",
            lxcup_core::OperatingSystem::Debian,
            lxcup_core::ContainerStatus::Running,
        )
        .unwrap();
        state.replace_containers(vec![container]).await;

        let create_request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/enrollments")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": 101,
                        "idempotency_key": "request-1"
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let first = router(state.clone())
            .oneshot(create_request())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let enrollment_id = first_json["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(first_json["data"]["state"], "requested");

        let second = router(state.clone())
            .oneshot(create_request())
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
        assert_eq!(second_json["data"]["id"], enrollment_id);

        let get = router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/enrollments/{enrollment_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn enrollment_rejects_key_reuse_and_unknown_containers() {
        let state = ApiState::new();
        let containers = [101, 102]
            .into_iter()
            .map(|id| {
                Container::new(
                    ContainerId::new(id),
                    NodeId::new(),
                    format!("test-lxc-{id}"),
                    lxcup_core::OperatingSystem::Debian,
                    lxcup_core::ContainerStatus::Running,
                )
                .unwrap()
            })
            .collect();
        state.replace_containers(containers).await;

        let request = |container_id, key| {
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/enrollments")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": container_id,
                        "idempotency_key": key
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let first = router(state.clone())
            .oneshot(request(101, "request-1"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);

        let reused = router(state.clone())
            .oneshot(request(102, "request-1"))
            .await
            .unwrap();
        assert_eq!(reused.status(), StatusCode::CONFLICT);

        let unknown = router(state)
            .oneshot(request(999, "request-2"))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn configured_auth_protects_api_but_not_health() {
        let state = ApiState::new().with_auth_config(
            AuthConfig::disabled()
                .with_tokens(Some("viewer".to_owned()), Some("operator".to_owned()), None)
                .required(true),
        );
        let api_response = router(state.clone())
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_response.status(), StatusCode::UNAUTHORIZED);

        let health_response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn agent_registration_exposes_health_and_metrics() {
        let agent_token = "test-agent-token";
        let agent_info = lxcup_agent::AgentInfo {
            agent_id: "test-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Linux,
            hostname: "test-lxc".to_owned(),
            version: "0.1.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let agent_task = tokio::spawn(async move {
            axum::serve(
                listener,
                lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(
                    agent_info,
                    agent_token,
                )),
            )
            .await
            .unwrap();
        });

        let state = ApiState::new().with_auth_config(AuthConfig::disabled());
        let container = Container::new(
            ContainerId::new(101),
            NodeId::new(),
            "test-lxc",
            lxcup_core::OperatingSystem::Debian,
            lxcup_core::ContainerStatus::Running,
        )
        .unwrap();
        state.replace_containers(vec![container]).await;

        let registration = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/101/agent")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "endpoint": format!("http://{address}"),
                    "token": agent_token,
                })
                .to_string(),
            ))
            .unwrap();
        let registration_response = router(state.clone()).oneshot(registration).await.unwrap();
        assert_eq!(registration_response.status(), StatusCode::CREATED);

        let health_response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/101/agent/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health_response.status(), StatusCode::OK);
        let health_body = axum::body::to_bytes(health_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let health_json: serde_json::Value = serde_json::from_slice(&health_body).unwrap();
        assert_eq!(health_json["data"]["healthy"], true);
        assert_eq!(health_json["data"]["info"]["agent_id"], "test-agent");

        let metrics_response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/101/agent/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics_response.status(), StatusCode::OK);
        let metrics_body = axum::body::to_bytes(metrics_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let metrics_json: serde_json::Value = serde_json::from_slice(&metrics_body).unwrap();
        assert_eq!(metrics_json["data"]["commands_total"], 0);
        assert_eq!(metrics_json["data"]["commands_failed"], 0);

        agent_task.abort();
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!(parse_container_id("0").is_err());
        assert!(parse_uuid("not-an-id", "plan id").is_err());
    }
}
