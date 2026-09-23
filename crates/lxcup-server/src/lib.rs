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
    extract::{Extension, Json as JsonBody, Path, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use lxcup_agent::{
    AgentAction, AgentClient, AgentClientConfig, AgentCommandRequest, AgentHealth, AgentHeartbeat,
    AgentMetrics, DockerContainerInfo,
};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobCoordinator, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation,
    AnsibleParameters, ExecutionMode, JobEvent, JobEventKind, JobFailureCode, JobSubmission,
};
use lxcup_core::{
    ActorRole, AgentRegistration, Container, ContainerAction, ContainerId,
    ContainerManagementState, DockerWorkload, DockerWorkloadManagementState, Enrollment,
    EnrollmentId, EnrollmentState, EnvironmentId, EnvironmentStatus, Execution, ExecutionId, Node,
    NodeId, Permission, PlanStatus, ProxmoxEnvironment, ResourceLifecycle, ResourceTarget, Scan,
    ScanId, SecretId, SecretKind, SecretScope, SecretValue, Target, TargetId, TargetKind,
    TargetState, TargetTransport, UpdatePlan, UpdatePlanId,
};
use lxcup_execution::{ExecutionCoordinator, ExecutionRequest, ExecutionStart};
use lxcup_persistence::Repositories;
use lxcup_planner::{DryRunChange, PlannerInput, UpdatePlanner};
use lxcup_safety::{HealthCheckResult, RebootRequirement, SnapshotState};
use lxcup_secrets::{
    CreateSecret, InMemorySecretStore, SecretStore, SecretStoreError, StoredSecretMetadata,
};
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
    ansible: Arc<RwLock<AnsibleJobCoordinator>>,
    secrets: Arc<dyn SecretStore>,
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
            ansible: Arc::new(RwLock::new(AnsibleJobCoordinator::default())),
            secrets: Arc::new(InMemorySecretStore::default()),
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

    pub fn with_secret_store(mut self, secrets: Arc<dyn SecretStore>) -> Self {
        self.secrets = secrets;
        self
    }

    pub async fn replace_nodes(&self, nodes: Vec<Node>) {
        self.store.write().await.nodes = nodes;
    }

    pub async fn replace_containers(&self, containers: Vec<Container>) {
        self.store.write().await.containers = containers;
    }

    /// Restores the platform-neutral target inventory after a server restart.
    pub async fn restore_targets(&self) -> usize {
        let Some(repositories) = self.repositories.as_ref() else {
            return 0;
        };
        let Ok(targets) = repositories.targets.list().await else {
            return 0;
        };
        let restored = targets.len();
        self.store.write().await.targets = targets;
        restored
    }

    /// Rebuilds active agent clients from persisted registrations after a
    /// restart. Secret values are resolved only for client construction and
    /// are never returned or published.
    pub async fn restore_registered_agents(&self) -> usize {
        let Some(repositories) = self.repositories.as_ref() else {
            return 0;
        };
        let Ok(registrations) = repositories.agent_registrations.list().await else {
            return 0;
        };
        let mut restored = 0;
        for registration in registrations {
            let Ok(secret) = self.secrets.read(registration.secret_ref) else {
                continue;
            };
            let Ok(config) = AgentClientConfig::new(registration.endpoint.clone(), secret.expose())
            else {
                continue;
            };
            let config = if let Some(ca_secret_ref) = registration.ca_secret_ref {
                let Ok(ca) = self.secrets.read(ca_secret_ref) else {
                    continue;
                };
                config.with_root_certificate_pem(ca.expose().as_bytes())
            } else {
                config
            };
            let Ok(client) = AgentClient::new(config) else {
                continue;
            };
            self.agents
                .write()
                .await
                .insert(registration.container_id, RegisteredAgent { client });
            restored += 1;
        }
        restored
    }

    /// Drops in-memory clients when their credential is rotated or revoked so
    /// the previous credential cannot be used through the lxcup API anymore.
    pub async fn invalidate_agents_for_secret(&self, secret_id: SecretId) {
        let Some(repositories) = self.repositories.as_ref() else {
            return;
        };
        let Ok(registrations) = repositories.agent_registrations.list().await else {
            return;
        };
        for registration in registrations
            .into_iter()
            .filter(|item| item.secret_ref == secret_id)
        {
            self.agents.write().await.remove(&registration.container_id);
            self.publish(ApiEvent::status(
                "agent",
                registration.container_id.value().to_string(),
                "credential_invalidated",
            ));
        }
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
    targets: Vec<Target>,
    agent_reports: HashMap<TargetId, AgentHeartbeat>,
    environments: Vec<ProxmoxEnvironment>,
    nodes: Vec<Node>,
    containers: Vec<Container>,
    enrollments: Vec<Enrollment>,
    enrollment_keys: HashMap<String, EnrollmentId>,
    scans: Vec<Scan>,
    plans: Vec<UpdatePlan>,
    executions: Vec<Execution>,
    safety: HashMap<ExecutionId, SafetyDto>,
    results: HashMap<ExecutionId, ExecutionResultDto>,
    container_actions: Vec<ContainerActionTask>,
    container_action_keys: HashMap<(ContainerId, String), Uuid>,
    secret_audit: Vec<SecretAuditEvent>,
    docker_workloads: HashMap<(ContainerId, String), DockerWorkloadDto>,
}

#[derive(Clone)]
struct RegisteredAgent {
    client: AgentClient,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateEnrollmentRequest {
    pub container_id: u64,
    #[serde(default)]
    pub target_id: Option<TargetId>,
    pub idempotency_key: String,
    #[serde(default = "default_true")]
    pub start_onboarding: bool,
}

fn default_true() -> bool {
    true
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
        .route("/api/v1/targets", get(list_targets).post(create_target))
        .route("/api/v1/targets/{target_id}", get(get_target))
        .route("/api/v1/agents/heartbeat", post(receive_agent_heartbeat))
        .route("/api/v1/nodes", get(list_nodes))
        .route(
            "/api/v1/environments",
            get(list_environments).post(create_environment),
        )
        .route(
            "/api/v1/environments/{environment_id}",
            get(get_environment)
                .patch(update_environment)
                .delete(delete_environment),
        )
        .route(
            "/api/v1/environments/{environment_id}/check",
            post(check_environment),
        )
        .route(
            "/api/v1/environments/{environment_id}/disable",
            post(disable_environment),
        )
        .route("/api/v1/secrets", get(list_secrets).post(create_secret))
        .route("/api/v1/secrets/audit", get(list_secret_audit))
        .route(
            "/api/v1/secrets/{secret_id}",
            get(get_secret).delete(delete_secret),
        )
        .route("/api/v1/secrets/{secret_id}/rotate", post(rotate_secret))
        .route("/api/v1/secrets/{secret_id}/revoke", post(revoke_secret))
        .route("/api/v1/enrollments", post(create_enrollment))
        .route("/api/v1/enrollments/{enrollment_id}", get(get_enrollment))
        .route(
            "/api/v1/ansible/jobs",
            get(list_ansible_jobs).post(create_ansible_job),
        )
        .route("/api/v1/ansible/jobs/{job_id}", get(get_ansible_job))
        .route(
            "/api/v1/ansible/jobs/{job_id}/events",
            get(get_ansible_job_events),
        )
        .route(
            "/api/v1/nodes/{node_id}/containers",
            get(list_node_containers),
        )
        .route("/api/v1/containers", get(list_containers))
        .route(
            "/api/v1/containers/{container_id}/actions",
            post(create_container_action),
        )
        .route(
            "/api/v1/container-actions/{action_id}",
            get(get_container_action),
        )
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
        .route(
            "/api/v1/containers/{container_id}/docker/containers",
            get(list_docker_containers),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/discover",
            post(discover_docker_containers),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/containers/{docker_id}/adopt",
            post(adopt_docker_container),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/containers/{docker_id}",
            axum::routing::delete(remove_docker_container),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/revoke",
            post(revoke_agent),
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

    fn forbidden(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message,
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

#[derive(Clone, Debug, Deserialize)]
pub struct CreateTargetRequest {
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    #[serde(default)]
    pub ssh_user: Option<String>,
    pub credential_secret_ref: SecretId,
    #[serde(default)]
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDto {
    pub id: TargetId,
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    pub ssh_user: Option<String>,
    pub credential_secret_ref: SecretId,
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
    pub state: TargetState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&Target> for TargetDto {
    fn from(target: &Target) -> Self {
        Self {
            id: target.id,
            name: target.name.clone(),
            kind: target.kind,
            address: target.address.clone(),
            transport: target.transport,
            ssh_user: target.ssh_user.clone(),
            credential_secret_ref: target.credential_secret_ref,
            ssh_known_hosts_secret_ref: target.ssh_known_hosts_secret_ref,
            agent_secret_ref: target.agent_secret_ref,
            state: target.state,
            created_at: target.created_at,
            updated_at: target.updated_at,
        }
    }
}

async fn list_targets(State(state): State<ApiState>) -> Json<ApiEnvelope<Vec<TargetDto>>> {
    let targets = state
        .store
        .read()
        .await
        .targets
        .iter()
        .map(TargetDto::from)
        .collect();
    Json(envelope(targets))
}

async fn create_target(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateTargetRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<TargetDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let mut target = Target::new(
        request.name,
        request.kind,
        request.address,
        request.transport,
        request.credential_secret_ref,
        request.agent_secret_ref,
    )
    .map_err(|_| {
        ApiError::bad_request("invalid_target", "target fields or transport are invalid")
    })?;
    target.ssh_user = request.ssh_user.filter(|value| !value.trim().is_empty());
    target.ssh_known_hosts_secret_ref = request.ssh_known_hosts_secret_ref;
    let dto = TargetDto::from(&target);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .save(&target)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.store.write().await.targets.push(target);
    state.publish(ApiEvent::status(
        "target",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

async fn get_target(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<TargetDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let store = state.store.read().await;
    let target = store
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    Ok(Json(envelope(TargetDto::from(target))))
}

async fn receive_agent_heartbeat(
    State(state): State<ApiState>,
    headers: HeaderMap,
    JsonBody(heartbeat): JsonBody<AgentHeartbeat>,
) -> Result<StatusCode, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let mut store = state.store.write().await;
    let target = store
        .targets
        .iter_mut()
        .find(|target| target.id.as_uuid() == heartbeat.target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    let expected = state
        .secrets
        .read(target.agent_secret_ref)
        .map_err(map_secret_error)?;
    if expected.expose() != token {
        return Err(ApiError::unauthorized());
    }
    target.mark_managed();
    let target_to_persist = target.clone();
    let target_id = target.id;
    store.agent_reports.insert(target_id, heartbeat);
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .update(&target_to_persist)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(ApiEvent::status(
        "target",
        target_id.as_uuid().to_string(),
        "agent_connected",
    ));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    pub endpoint: String,
    pub api_secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateEnvironmentRequest {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub api_secret_ref: Option<SecretId>,
    pub ca_secret_ref: Option<SecretId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConfirmedRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentDto {
    pub id: EnvironmentId,
    pub name: String,
    pub endpoint: String,
    pub api_secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
    pub status: EnvironmentStatus,
    pub last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_check_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ProxmoxEnvironment> for EnvironmentDto {
    fn from(environment: &ProxmoxEnvironment) -> Self {
        Self {
            id: environment.id,
            name: environment.name.clone(),
            endpoint: environment.endpoint.clone(),
            api_secret_ref: environment.api_secret_ref,
            ca_secret_ref: environment.ca_secret_ref,
            status: environment.status,
            last_checked_at: environment.last_checked_at,
            last_check_error: environment.last_check_error.clone(),
            created_at: environment.created_at,
            updated_at: environment.updated_at,
        }
    }
}

async fn list_environments(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<EnvironmentDto>>>, ApiError> {
    let environments = if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.environments.clone()
    };
    Ok(Json(envelope(
        environments.iter().map(EnvironmentDto::from).collect(),
    )))
}

async fn create_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<EnvironmentDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let now = chrono::Utc::now();
    let environment = ProxmoxEnvironment::new(
        EnvironmentId::new(),
        request.name,
        request.endpoint,
        request.api_secret_ref,
        request.ca_secret_ref,
        now,
    )
    .map_err(map_environment_domain_error)?;
    let dto = EnvironmentDto::from(&environment);
    let store = state.store.read().await;
    if store
        .environments
        .iter()
        .any(|item| item.name.eq_ignore_ascii_case(&environment.name))
    {
        return Err(ApiError::conflict(
            "environment_name_exists",
            "an environment with this name already exists",
        ));
    }
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .save(&environment)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    let mut store = state.store.write().await;
    store.environments.push(environment);
    drop(store);
    state.publish(ApiEvent::status(
        "environment",
        dto.id.as_uuid().to_string(),
        "configured",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

async fn get_environment(
    State(state): State<ApiState>,
    Path(environment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    let id = parse_environment_id(&environment_id)?;
    let store = state.store.read().await;
    let environment = store
        .environments
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("environment not found"))?;
    Ok(Json(envelope(EnvironmentDto::from(&*environment))))
}

async fn update_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<UpdateEnvironmentRequest>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let updated = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        let name = request.name.unwrap_or_else(|| environment.name.clone());
        let endpoint = request
            .endpoint
            .unwrap_or_else(|| environment.endpoint.clone());
        let api_secret_ref = request.api_secret_ref.unwrap_or(environment.api_secret_ref);
        environment
            .update_configuration(
                name,
                endpoint,
                api_secret_ref,
                request.ca_secret_ref.or(environment.ca_secret_ref),
                now,
            )
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&updated)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&updated))))
}

async fn disable_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<ConfirmedRequest>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "disabling an environment requires explicit confirmation",
        ));
    }
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let disabled = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        environment
            .transition_to(EnvironmentStatus::Disabled, now)
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&disabled)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&disabled))))
}

async fn delete_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<ConfirmedRequest>,
) -> Result<StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "deleting an environment requires explicit confirmation",
        ));
    }
    let id = parse_environment_id(&environment_id)?;
    let mut store = state.store.write().await;
    let position = store
        .environments
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("environment not found"))?;
    store.environments.remove(position);
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .delete(id)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn check_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Read)?;
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let checked = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        // The adapter/secret-store call is intentionally a separate worker seam.
        // Until that seam is wired, record a diagnosable, non-successful check.
        environment
            .record_check_failure(now, "Proxmox capability check is not scheduled")
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&checked)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&checked))))
}

fn require_permission(role: ActorRole, permission: Permission) -> Result<(), ApiError> {
    if role.grants(permission) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "permission_denied",
            "the role cannot change this resource",
        ))
    }
}

fn map_environment_domain_error(error: lxcup_core::DomainError) -> ApiError {
    match error {
        lxcup_core::DomainError::PermissionDenied => {
            ApiError::forbidden("permission_denied", "the role cannot change this resource")
        }
        lxcup_core::DomainError::EmptyValue { .. }
        | lxcup_core::DomainError::InvalidEnvironmentEndpoint
        | lxcup_core::DomainError::InvalidPackageName => ApiError::bad_request(
            "invalid_environment",
            "the environment configuration is invalid",
        ),
        lxcup_core::DomainError::ConfirmationRequired => {
            ApiError::bad_request("confirmation_required", "explicit confirmation is required")
        }
        lxcup_core::DomainError::InvalidStateTransition(_) => ApiError::conflict(
            "invalid_environment_state",
            "the environment state cannot change",
        ),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerActionStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Blocked,
}

#[derive(Clone, Debug)]
struct ContainerActionTask {
    id: Uuid,
    container_id: ContainerId,
    action: ContainerAction,
    status: ContainerActionStatus,
    idempotency_key: String,
    error: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateContainerActionRequest {
    pub action: ContainerAction,
    pub idempotency_key: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerActionTaskDto {
    pub id: Uuid,
    pub container_id: ContainerId,
    pub action: ContainerAction,
    pub status: ContainerActionStatus,
    pub idempotency_key: String,
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ContainerActionTask> for ContainerActionTaskDto {
    fn from(task: &ContainerActionTask) -> Self {
        Self {
            id: task.id,
            container_id: task.container_id,
            action: task.action,
            status: task.status,
            idempotency_key: task.idempotency_key.clone(),
            error: task.error.clone(),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

async fn create_container_action(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<CreateContainerActionRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ContainerActionTaskDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if request.action.requires_confirmation() {
        require_permission(actor_role, Permission::Destructive)?;
    }
    let container_id = parse_container_id(&container_id)?;
    let mut store = state.store.write().await;
    let container = store
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("container not found"))?;
    container
        .validate_action(request.action, request.confirmed, &request.idempotency_key)
        .map_err(map_container_action_error)?;

    let key = (container_id, request.idempotency_key.clone());
    if let Some(existing_id) = store.container_action_keys.get(&key).copied() {
        let existing = store
            .container_actions
            .iter()
            .find(|task| task.id == existing_id)
            .expect("container action idempotency index must point to a task");
        if existing.action != request.action {
            return Err(ApiError::conflict(
                "idempotency_key_reused",
                "idempotency key is already used for another action",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(envelope(ContainerActionTaskDto::from(existing))),
        ));
    }

    if store.container_actions.iter().any(|task| {
        task.container_id == container_id && task.status == ContainerActionStatus::Running
    }) {
        return Err(ApiError::conflict(
            "container_action_busy",
            "another action is already running for this container",
        ));
    }

    let now = chrono::Utc::now();
    let task = ContainerActionTask {
        id: Uuid::new_v4(),
        container_id,
        action: request.action,
        status: ContainerActionStatus::Queued,
        idempotency_key: request.idempotency_key,
        error: None,
        created_at: now,
        updated_at: now,
    };
    let dto = ContainerActionTaskDto::from(&task);
    store
        .container_action_keys
        .insert((container_id, task.idempotency_key.clone()), task.id);
    store.container_actions.push(task);
    drop(store);
    state.publish(ApiEvent::status(
        "container_action",
        dto.id.to_string(),
        "queued",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

async fn get_container_action(
    State(state): State<ApiState>,
    Path(action_id): Path<String>,
) -> Result<Json<ApiEnvelope<ContainerActionTaskDto>>, ApiError> {
    let action_id = parse_uuid(&action_id, "container action id")?;
    let store = state.store.read().await;
    let task = store
        .container_actions
        .iter()
        .find(|task| task.id == action_id)
        .ok_or_else(|| ApiError::not_found("container action not found"))?;
    Ok(Json(envelope(ContainerActionTaskDto::from(task))))
}

fn map_container_action_error(error: lxcup_core::DomainError) -> ApiError {
    match error {
        lxcup_core::DomainError::ConfirmationRequired => ApiError::bad_request(
            "confirmation_required",
            "the container action requires explicit confirmation",
        ),
        lxcup_core::DomainError::EmptyValue { .. }
        | lxcup_core::DomainError::InvalidPackageName
        | lxcup_core::DomainError::InvalidEnvironmentEndpoint => ApiError::bad_request(
            "invalid_container_action",
            "the container action is invalid",
        ),
        lxcup_core::DomainError::InvalidStateTransition(_) => ApiError::conflict(
            "container_action_invalid_state",
            "the container status does not allow this action",
        ),
        lxcup_core::DomainError::PermissionDenied => {
            ApiError::forbidden("permission_denied", "the role cannot run this action")
        }
    }
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

    // When the worker credential references are configured, enrollment also
    // creates the allowlisted deployment job. Development instances without
    // worker configuration still expose the lifecycle request for contract
    // testing; no shell or free-form playbook path is ever accepted here.
    if request.start_onboarding && request.target_id.is_some() {
        let target_id = request.target_id.expect("target id checked above");
        let target = state
            .store
            .read()
            .await
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?;
        let lifecycle = match target.state {
            TargetState::Pending => ResourceLifecycle::Pending,
            TargetState::Managed => ResourceLifecycle::Managed,
            TargetState::Disabled => ResourceLifecycle::Disabled,
        };
        let job_request = AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
            secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
            idempotency_key: format!("enrollment-{}", dto.id.as_uuid()),
            confirmed: true,
            actor_role: ActorRole::Operator,
        };
        let submission = state
            .ansible
            .write()
            .await
            .submit(job_request)
            .map_err(|_| {
                ApiError::conflict(
                    "enrollment_job_rejected",
                    "agent deployment could not be queued",
                )
            })?;
        let is_created = matches!(&submission, JobSubmission::Created(_));
        let job = match submission {
            JobSubmission::Created(job) | JobSubmission::Duplicate(job) => job,
        };
        if is_created {
            if let Some(repositories) = state.repositories.clone() {
                repositories
                    .ansible_jobs
                    .save(&job)
                    .await
                    .map_err(|_| ApiError::storage())?;
            }
            let mut store = state.store.write().await;
            if let Some(enrollment) = store.enrollments.iter_mut().find(|item| item.id == dto.id) {
                enrollment
                    .transition_to(EnrollmentState::Discovering)
                    .map_err(|_| {
                        ApiError::conflict(
                            "enrollment_invalid_state",
                            "enrollment cannot start discovery",
                        )
                    })?;
            }
            state.publish(ApiEvent::status(
                "ansible_job",
                job.id.as_uuid().to_string(),
                "queued",
            ));
        }
    }

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
    let enrollment_exists = state
        .store
        .read()
        .await
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .is_some();
    if !enrollment_exists {
        return Err(ApiError::not_found("enrollment not found"));
    }

    // Reconcile the enrollment view with the deployment job. This keeps the
    // onboarding page from waiting forever when the worker has already
    // finished (or rejected) the linked agent deployment.
    let job_key = format!("enrollment-{}", enrollment_id.as_uuid());
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    if let Some(job) = jobs.iter().find(|job| job.idempotency_key == job_key) {
        let target_connected = if let ResourceTarget::Target(target_id) = job.target {
            state
                .store
                .read()
                .await
                .targets
                .iter()
                .any(|target| target.id == target_id && target.state == TargetState::Managed)
        } else {
            false
        };
        let mut store = state.store.write().await;
        if let Some(current) = store
            .enrollments
            .iter_mut()
            .find(|item| item.id == enrollment_id)
        {
            match job.status {
                AnsibleJobStatus::Failed => current
                    .fail("Agent-Deployment fehlgeschlagen. Details stehen im Workflow-Protokoll."),
                AnsibleJobStatus::Applying | AnsibleJobStatus::Succeeded
                    if current.state == EnrollmentState::Discovering =>
                {
                    let _ = current.transition_to(EnrollmentState::InstallingAgent);
                }
                AnsibleJobStatus::Succeeded if target_connected => {
                    let _ = current.transition_to(EnrollmentState::RegisteringAgent);
                    let _ = current.transition_to(EnrollmentState::Connected);
                }
                _ => {}
            }
        }
    }
    let store = state.store.read().await;
    let enrollment = store
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .ok_or_else(|| ApiError::not_found("enrollment not found"))?;
    Ok(Json(envelope(EnrollmentDto::from(enrollment))))
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateAnsibleJobRequest {
    pub operation: AnsibleOperation,
    #[serde(default)]
    pub target_id: Option<TargetId>,
    #[serde(default)]
    pub container_id: u64,
    pub mode: ExecutionMode,
    pub parameters: AnsibleParameters,
    pub idempotency_key: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnsibleJobDto {
    pub id: lxcup_core::AnsibleJobId,
    pub operation: AnsibleOperation,
    pub playbook: String,
    pub playbook_version: String,
    pub target: ResourceTarget,
    pub mode: ExecutionMode,
    pub status: AnsibleJobStatus,
    pub parameter_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&AnsibleJob> for AnsibleJobDto {
    fn from(job: &AnsibleJob) -> Self {
        Self {
            id: job.id,
            operation: job.operation,
            playbook: job.playbook.clone(),
            playbook_version: job.playbook_version.clone(),
            target: job.target,
            mode: job.mode,
            status: job.status,
            parameter_hash: job.parameter_hash.clone(),
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

async fn list_ansible_jobs(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<AnsibleJobDto>>>, ApiError> {
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    Ok(Json(envelope(
        jobs.iter().map(AnsibleJobDto::from).collect(),
    )))
}

async fn create_ansible_job(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateAnsibleJobRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<AnsibleJobDto>>), ApiError> {
    let (target, lifecycle, target_secret_ref) = if let Some(target_id) = request.target_id {
        let target = state
            .store
            .read()
            .await
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?;
        let lifecycle = match target.state {
            TargetState::Pending => ResourceLifecycle::Pending,
            TargetState::Managed => ResourceLifecycle::Managed,
            TargetState::Disabled => ResourceLifecycle::Disabled,
        };
        (
            ResourceTarget::Target(target_id),
            lifecycle,
            Some(target.credential_secret_ref),
        )
    } else {
        if request.container_id == 0 {
            return Err(ApiError::bad_request(
                "target_required",
                "target id must be provided",
            ));
        }
        let container_id = ContainerId::new(request.container_id);
        let lifecycle = state
            .store
            .read()
            .await
            .containers
            .iter()
            .find(|container| container.id == container_id)
            .map(|container| match container.management_state {
                ContainerManagementState::Discovered => ResourceLifecycle::Discovered,
                ContainerManagementState::Managed => ResourceLifecycle::Managed,
                ContainerManagementState::Ignored | ContainerManagementState::Disabled => {
                    ResourceLifecycle::Disabled
                }
            })
            .ok_or_else(|| ApiError::not_found("container not found"))?;
        (ResourceTarget::Container(container_id), lifecycle, None)
    };

    let secret_refs = if request.operation == AnsibleOperation::HealthCheck {
        Vec::new()
    } else {
        target_secret_ref.map_or_else(configured_ansible_secret_refs, |secret| Ok(vec![secret]))?
    };
    let job_request = AnsibleJobRequest {
        operation: request.operation,
        target,
        lifecycle,
        mode: request.mode,
        parameters: request.parameters,
        secret_refs,
        idempotency_key: request.idempotency_key,
        confirmed: request.confirmed,
        actor_role,
    };
    let target = job_request.target;
    if let Some(repositories) = state.repositories.clone() {
        if let Some(existing) = repositories
            .ansible_jobs
            .find_by_idempotency_key(target, &job_request.idempotency_key)
            .await
            .map_err(|_| ApiError::storage())?
        {
            return Ok((
                StatusCode::OK,
                Json(envelope(AnsibleJobDto::from(&existing))),
            ));
        }
        if repositories
            .ansible_jobs
            .has_active_target(target)
            .await
            .map_err(|_| ApiError::storage())?
        {
            return Err(ApiError::conflict(
                "ansible_target_busy",
                "another job is active for this target",
            ));
        }
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(job_request)
        .map_err(map_ansible_error)?;
    let (status, job, created) = match submission {
        JobSubmission::Created(job) => (StatusCode::ACCEPTED, job, true),
        JobSubmission::Duplicate(job) => (StatusCode::OK, job, false),
    };
    if created {
        if let Some(repositories) = state.repositories.clone() {
            repositories
                .ansible_jobs
                .save(&job)
                .await
                .map_err(|_| ApiError::storage())?;
            let events = state
                .ansible
                .read()
                .await
                .events(job.id)
                .map_err(map_ansible_error)?;
            for event in events {
                repositories
                    .ansible_jobs
                    .append_event(&event)
                    .await
                    .map_err(|_| ApiError::storage())?;
            }
        }
    }
    let dto = AnsibleJobDto::from(&job);
    state.publish(ApiEvent::status(
        "ansible_job",
        dto.id.as_uuid().to_string(),
        "queued",
    ));
    Ok((status, Json(envelope(dto))))
}

async fn get_ansible_job(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiEnvelope<AnsibleJobDto>>, ApiError> {
    let id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    let job = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .find_by_id(id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("ansible job not found"))?
    } else {
        state
            .ansible
            .read()
            .await
            .job(id)
            .map_err(map_ansible_error)?
    };
    Ok(Json(envelope(AnsibleJobDto::from(&job))))
}

async fn get_ansible_job_events(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<lxcup_ansible::JobEvent>>>, ApiError> {
    let id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    if let Some(repositories) = state.repositories.clone() {
        let mut events = repositories
            .ansible_jobs
            .events(id)
            .await
            .map_err(|_| ApiError::storage())?;
        let job = repositories
            .ansible_jobs
            .find_by_id(id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("ansible job not found"))?;
        if job.status == AnsibleJobStatus::Failed
            && !events
                .iter()
                .any(|event| matches!(&event.event, JobEventKind::Failed { .. }))
        {
            let event = JobEvent {
                sequence: events.len() as u64 + 1,
                job_id: id,
                event: JobEventKind::Failed {
                    code: JobFailureCode::WorkerUnavailable,
                },
                created_at: chrono::Utc::now(),
            };
            repositories
                .ansible_jobs
                .append_event(&event)
                .await
                .map_err(|_| ApiError::storage())?;
            events.push(event);
        }
        Ok(Json(envelope(events)))
    } else {
        state
            .ansible
            .read()
            .await
            .events(id)
            .map(|events| Json(envelope(events)))
            .map_err(map_ansible_error)
    }
}

fn configured_ansible_secret_refs() -> Result<Vec<SecretId>, ApiError> {
    let raw = std::env::var("LXCUP_ANSIBLE_SECRET_IDS").map_err(|_| {
        ApiError::dependency(
            "secret_reference_missing",
            "no Ansible credential is configured",
        )
    })?;
    let refs = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Uuid::parse_str(value)
                .map(SecretId::from_uuid)
                .map_err(|_| {
                    ApiError::bad_request("invalid_secret_reference", "secret reference is invalid")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if refs.is_empty() {
        return Err(ApiError::dependency(
            "secret_reference_missing",
            "no Ansible credential is configured",
        ));
    }
    Ok(refs)
}

fn map_ansible_error(error: lxcup_ansible::CoordinatorError) -> ApiError {
    match error {
        lxcup_ansible::CoordinatorError::TargetBusy => ApiError::conflict(
            "ansible_target_busy",
            "another job is active for this target",
        ),
        lxcup_ansible::CoordinatorError::NotFound => ApiError::not_found("ansible job not found"),
        lxcup_ansible::CoordinatorError::RetryNotAllowed
        | lxcup_ansible::CoordinatorError::InvalidTransition
        | lxcup_ansible::CoordinatorError::Terminal => {
            ApiError::bad_request("ansible_job_invalid", "the Ansible job state is invalid")
        }
        lxcup_ansible::CoordinatorError::Contract(error) => match error {
            lxcup_ansible::AnsibleContractError::PermissionDenied => ApiError::forbidden(
                "ansible_permission_denied",
                "the role cannot run this operation",
            ),
            lxcup_ansible::AnsibleContractError::ConfirmationRequired => ApiError::bad_request(
                "ansible_confirmation_required",
                "the operation requires explicit confirmation",
            ),
            lxcup_ansible::AnsibleContractError::MissingSecretReference => ApiError::dependency(
                "secret_reference_missing",
                "the operation has no configured credential reference",
            ),
            lxcup_ansible::AnsibleContractError::UnsupportedTarget => ApiError::bad_request(
                "ansible_target_invalid",
                "the operation is not supported for this target",
            ),
            lxcup_ansible::AnsibleContractError::Invalid
            | lxcup_ansible::AnsibleContractError::InvalidTransition => {
                ApiError::bad_request("ansible_request_invalid", "the Ansible request is invalid")
            }
        },
    }
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
        self.role(headers).is_some_and(|role| {
            role == lxcup_core::ActorRole::Admin
                || role == lxcup_core::ActorRole::Operator
                || (method == Method::GET && role == lxcup_core::ActorRole::Viewer)
        })
    }

    fn role(&self, headers: &HeaderMap) -> Option<ActorRole> {
        if !self.required {
            return Some(ActorRole::Admin);
        }
        let Some(token) = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return None;
        };
        if self.admin_token.as_deref() == Some(token) {
            Some(ActorRole::Admin)
        } else if self.operator_token.as_deref() == Some(token) {
            Some(ActorRole::Operator)
        } else if self.viewer_token.as_deref() == Some(token) {
            Some(ActorRole::Viewer)
        } else {
            None
        }
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
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let public = path == "/health/live" || path == "/health/ready" || path == "/metrics";
    if !public && !state.auth.allows(request.method(), request.headers()) {
        return ApiError::unauthorized().into_response();
    }
    if let Some(role) = state.auth.role(request.headers()) {
        request.extensions_mut().insert(role);
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
    #[serde(default)]
    pub secret_ref: Option<SecretId>,
    #[serde(default)]
    pub ca_secret_ref: Option<SecretId>,
    /// Development-only compatibility input. Production requests must use
    /// `secret_ref`; this field is rejected unless explicitly enabled.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentRegistrationDto {
    pub endpoint: String,
    pub info: AgentHealth,
}

async fn register_agent(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<RegisterAgentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<AgentRegistrationDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
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
    let secret_ref = request.secret_ref;
    let token = if let Some(secret_ref) = secret_ref {
        state
            .secrets
            .read(secret_ref)
            .map_err(map_secret_error)?
            .expose()
            .to_owned()
    } else if std::env::var("LXCUP_ALLOW_LEGACY_AGENT_TOKEN").as_deref() == Ok("true") {
        request.token.ok_or_else(|| {
            ApiError::bad_request(
                "agent_secret_required",
                "agent registration requires a secret reference",
            )
        })?
    } else {
        return Err(ApiError::bad_request(
            "agent_secret_required",
            "agent registration requires a secret reference",
        ));
    };
    let config = AgentClientConfig::new(request.endpoint.clone(), token).map_err(|_| {
        ApiError::bad_request("invalid_agent_config", "agent endpoint or token is invalid")
    })?;
    let config = if let Some(ca_secret_ref) = request.ca_secret_ref {
        let ca = state
            .secrets
            .read(ca_secret_ref)
            .map_err(map_secret_error)?;
        config.with_root_certificate_pem(ca.expose().as_bytes())
    } else {
        config
    };
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
    if let Some(secret_ref) = secret_ref {
        let registration = AgentRegistration::new(
            container_id,
            info.info.agent_id.clone(),
            request.endpoint.clone(),
            secret_ref,
            request.ca_secret_ref,
            chrono::Utc::now(),
        )
        .map_err(|_| {
            ApiError::bad_request(
                "invalid_agent_registration",
                "agent registration is invalid",
            )
        })?;
        if let Some(repositories) = state.repositories.as_ref() {
            repositories
                .agent_registrations
                .save(&registration)
                .await
                .map_err(|_| {
                    ApiError::dependency(
                        "persistence_unavailable",
                        "agent registration could not be persisted",
                    )
                })?;
        }
    }
    state.publish(ApiEvent::status(
        "agent",
        container_id.value().to_string(),
        "registered",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

fn map_secret_error(error: SecretStoreError) -> ApiError {
    match error {
        SecretStoreError::Missing => ApiError::dependency(
            "agent_secret_missing",
            "the configured agent secret is missing",
        ),
        SecretStoreError::Denied => ApiError::forbidden(
            "agent_secret_denied",
            "the configured agent secret is not usable",
        ),
        SecretStoreError::Invalid => ApiError::bad_request(
            "agent_secret_invalid",
            "the configured agent secret is invalid",
        ),
        SecretStoreError::Unavailable => ApiError::dependency(
            "secret_store_unavailable",
            "the secret store is unavailable",
        ),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub kind: SecretKind,
    pub scope: SecretScope,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SecretValueRequest {
    pub value: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SecretConfirmationRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SecretMetadataDto {
    pub metadata: StoredSecretMetadata,
}

#[derive(Clone, Debug, Serialize)]
pub struct SecretAuditEvent {
    pub secret_id: SecretId,
    pub action: String,
    pub role: ActorRole,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

async fn list_secrets(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<SecretMetadataDto>>>, ApiError> {
    let secrets = state
        .secrets
        .list_metadata()
        .map_err(map_secret_error)?
        .into_iter()
        .map(|metadata| SecretMetadataDto { metadata })
        .collect();
    Ok(Json(envelope(secrets)))
}

async fn create_secret(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateSecretRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<SecretMetadataDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let value = SecretValue::new(request.value)
        .map_err(|_| ApiError::bad_request("invalid_secret", "the secret value is invalid"))?;
    let metadata = state
        .secrets
        .create(CreateSecret {
            name: request.name,
            kind: request.kind,
            scope: request.scope,
            value,
        })
        .map_err(map_secret_error)?;
    let id = metadata.metadata.id;
    record_secret_audit(&state, id, "created", actor_role).await;
    Ok((
        StatusCode::CREATED,
        Json(envelope(SecretMetadataDto { metadata })),
    ))
}

async fn list_secret_audit(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<SecretAuditEvent>>> {
    Json(envelope(state.store.read().await.secret_audit.clone()))
}

async fn get_secret(
    State(state): State<ApiState>,
    Path(secret_id): Path<String>,
) -> Result<Json<ApiEnvelope<SecretMetadataDto>>, ApiError> {
    let id = parse_secret_id(&secret_id)?;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

async fn rotate_secret(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(secret_id): Path<String>,
    JsonBody(request): JsonBody<SecretValueRequest>,
) -> Result<Json<ApiEnvelope<SecretMetadataDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "rotating a secret requires explicit confirmation",
        ));
    }
    let id = parse_secret_id(&secret_id)?;
    let value = SecretValue::new(request.value)
        .map_err(|_| ApiError::bad_request("invalid_secret", "the secret value is invalid"))?;
    state.secrets.rotate(id, value).map_err(map_secret_error)?;
    state.invalidate_agents_for_secret(id).await;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    record_secret_audit(&state, id, "rotated", actor_role).await;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

async fn revoke_secret(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(secret_id): Path<String>,
    JsonBody(request): JsonBody<SecretConfirmationRequest>,
) -> Result<Json<ApiEnvelope<SecretMetadataDto>>, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "revoking a secret requires explicit confirmation",
        ));
    }
    let id = parse_secret_id(&secret_id)?;
    state.secrets.revoke(id).map_err(map_secret_error)?;
    state.invalidate_agents_for_secret(id).await;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    record_secret_audit(&state, id, "revoked", actor_role).await;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

async fn delete_secret(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(secret_id): Path<String>,
    JsonBody(request): JsonBody<SecretConfirmationRequest>,
) -> Result<StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "deleting a secret requires explicit confirmation",
        ));
    }
    let id = parse_secret_id(&secret_id)?;
    state.secrets.delete(id).map_err(map_secret_error)?;
    record_secret_audit(&state, id, "deleted", actor_role).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn record_secret_audit(state: &ApiState, id: SecretId, action: &str, role: ActorRole) {
    let event = SecretAuditEvent {
        secret_id: id,
        action: action.to_owned(),
        role,
        occurred_at: chrono::Utc::now(),
    };
    state.store.write().await.secret_audit.push(event);
    state.publish(ApiEvent::status("secret", id.as_uuid().to_string(), action));
}

fn parse_secret_id(value: &str) -> Result<SecretId, ApiError> {
    Ok(SecretId::from_uuid(parse_uuid(value, "secret id")?))
}

async fn revoke_agent(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<ConfirmedRequest>,
) -> Result<StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "revoking an agent requires explicit confirmation",
        ));
    }
    let container_id = parse_container_id(&container_id)?;
    let removed = state.agents.write().await.remove(&container_id);
    if removed.is_none() {
        return Err(ApiError::not_found("agent not registered"));
    }
    state.publish(ApiEvent::status(
        "agent",
        container_id.value().to_string(),
        "revoked",
    ));
    Ok(StatusCode::NO_CONTENT)
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
    let health = match agent.client.health().await {
        Ok(health) => health,
        Err(_) => {
            if let Some(repositories) = state.repositories.as_ref() {
                if let Ok(Some(mut registration)) = repositories
                    .agent_registrations
                    .find_by_container(container_id)
                    .await
                {
                    registration.record_unreachable(chrono::Utc::now(), "agent healthcheck failed");
                    let _ = repositories.agent_registrations.save(&registration).await;
                }
            }
            state.publish(ApiEvent::status(
                "agent",
                container_id.value().to_string(),
                "unreachable",
            ));
            return Err(ApiError::dependency(
                "agent_unavailable",
                "the agent healthcheck failed",
            ));
        }
    };
    if let Some(repositories) = state.repositories.as_ref() {
        if let Ok(Some(mut registration)) = repositories
            .agent_registrations
            .find_by_container(container_id)
            .await
        {
            registration.record_health(health.healthy, chrono::Utc::now(), None);
            let _ = repositories.agent_registrations.save(&registration).await;
        }
    }
    state.publish(ApiEvent::status(
        "agent",
        container_id.value().to_string(),
        if health.healthy {
            "connected"
        } else {
            "degraded"
        },
    ));
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

#[derive(Clone, Debug, Serialize)]
pub struct DockerWorkloadDto {
    pub host_container_id: ContainerId,
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub management_state: String,
    pub discovered_at: chrono::DateTime<chrono::Utc>,
}

impl DockerWorkloadDto {
    fn discovered(host_container_id: ContainerId, container: DockerContainerInfo) -> Self {
        Self {
            host_container_id,
            id: container.id,
            name: container.name,
            image: container.image,
            state: container.state,
            status: container.status,
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
            management_state: match item.management_state {
                DockerWorkloadManagementState::Discovered => "discovered".to_owned(),
                DockerWorkloadManagementState::Managed => "managed".to_owned(),
            },
            discovered_at: item.discovered_at,
        }
    }
}

async fn list_docker_containers(
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

async fn discover_docker_containers(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<DockerWorkloadDto>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let agent = state
        .agents
        .read()
        .await
        .get(&container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("agent not registered"))?;
    let discovered = agent.client.docker_containers().await.map_err(|_| {
        ApiError::dependency(
            "docker_discovery_failed",
            "Docker-Inventar konnte vom Agenten nicht gelesen werden",
        )
    })?;
    if let Some(repositories) = state.repositories.as_ref() {
        for container in &discovered {
            let workload = DockerWorkload {
                host_container_id: container_id,
                id: container.id.clone(),
                name: container.name.clone(),
                image: container.image.clone(),
                state: container.state.clone(),
                status: container.status.clone(),
                management_state: DockerWorkloadManagementState::Discovered,
                discovered_at: chrono::Utc::now(),
            };
            repositories
                .docker_workloads
                .upsert_discovered(&workload)
                .await
                .map_err(|_| {
                    ApiError::dependency(
                        "docker_inventory_unavailable",
                        "Docker-Inventar konnte nicht gespeichert werden",
                    )
                })?;
        }
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
        state.publish(ApiEvent::status(
            "docker",
            container_id.value().to_string(),
            "discovered",
        ));
        return Ok(Json(envelope(workloads)));
    }
    let mut store = state.store.write().await;
    for container in discovered {
        let key = (container_id, container.id.clone());
        let management_state = store
            .docker_workloads
            .get(&key)
            .map(|item| item.management_state.clone())
            .unwrap_or_else(|| "discovered".to_owned());
        let mut workload = DockerWorkloadDto::discovered(container_id, container);
        workload.management_state = management_state;
        store.docker_workloads.insert(key, workload);
    }
    let workloads = store
        .docker_workloads
        .values()
        .filter(|item| item.host_container_id == container_id)
        .cloned()
        .collect();
    drop(store);
    state.publish(ApiEvent::status(
        "docker",
        container_id.value().to_string(),
        "discovered",
    ));
    Ok(Json(envelope(workloads)))
}

async fn adopt_docker_container(
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
struct RemoveDockerWorkloadRequest {
    confirmed: bool,
}

async fn remove_docker_container(
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
    "/api/v1/ansible/jobs": {"post": {"responses": {"202": {"description": "Ansible job accepted"}}}},
    "/api/v1/ansible/jobs/{job_id}": {"get": {"responses": {"200": {"description": "Ansible job status"}}}},
    "/api/v1/ansible/jobs/{job_id}/events": {"get": {"responses": {"200": {"description": "Audit-safe Ansible job events"}}}},
    "/api/v1/secrets": {"get": {}, "post": {"description": "Create or list secret metadata; values are never returned"}},
    "/api/v1/secrets/{secret_id}": {"get": {}, "delete": {}},
    "/api/v1/secrets/{secret_id}/rotate": {"post": {}},
    "/api/v1/secrets/{secret_id}/revoke": {"post": {}},
    "/api/v1/secrets/audit": {"get": {}},
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

fn parse_environment_id(value: &str) -> Result<EnvironmentId, ApiError> {
    Ok(EnvironmentId::from_uuid(parse_uuid(
        value,
        "environment id",
    )?))
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
    use lxcup_secrets::CreateSecret;
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
    async fn ansible_job_endpoint_accepts_healthcheck_and_is_idempotent() {
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
        let request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/ansible/jobs")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "operation": "health_check",
                        "container_id": 101,
                        "mode": "check",
                        "parameters": {"operation": "health_check"},
                        "idempotency_key": "health-1",
                        "confirmed": false
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let first = router(state.clone()).oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let job_id = first_json["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(first_json["data"]["status"], "queued");

        let duplicate = router(state.clone()).oneshot(request()).await.unwrap();
        assert_eq!(duplicate.status(), StatusCode::OK);
        let duplicate_body = axum::body::to_bytes(duplicate.into_body(), usize::MAX)
            .await
            .unwrap();
        let duplicate_json: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
        assert_eq!(duplicate_json["data"]["id"], job_id);

        let events = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/ansible/jobs/{job_id}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);

        let status = router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/ansible/jobs/{job_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
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

        let secret_store = InMemorySecretStore::default();
        let agent_secret = secret_store
            .create(CreateSecret {
                name: "test-agent".to_owned(),
                kind: lxcup_core::SecretKind::AgentToken,
                scope: lxcup_core::SecretScope::Container(ContainerId::new(101)),
                value: lxcup_core::SecretValue::new(agent_token).unwrap(),
            })
            .unwrap();
        let state = ApiState::new()
            .with_auth_config(AuthConfig::disabled())
            .with_secret_store(Arc::new(secret_store));
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
                    "secret_ref": agent_secret.metadata.id,
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

        let metrics_response = router(state.clone())
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

        let revoke = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/101/agent/revoke")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap();
        assert_eq!(
            router(state).oneshot(revoke).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );

        agent_task.abort();
    }

    #[tokio::test]
    async fn environment_api_keeps_secret_values_out_of_responses_and_supports_lifecycle() {
        let state = ApiState::new().with_auth_config(AuthConfig::disabled());
        let create = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/environments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "pve-test",
                    "endpoint": "https://pve.example.test/",
                    "api_secret_ref": SecretId::new(),
                    "ca_secret_ref": null
                })
                .to_string(),
            ))
            .unwrap();
        let response = router(state.clone()).oneshot(create).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = json["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(json["data"]["endpoint"], "https://pve.example.test");
        assert!(!json.to_string().contains("token_secret"));

        let update = Request::builder()
            .method(Method::PATCH)
            .uri(format!("/api/v1/environments/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"name": "pve-renamed"}).to_string(),
            ))
            .unwrap();
        assert_eq!(
            router(state.clone())
                .oneshot(update)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let check = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/environments/{id}/check"))
            .body(Body::empty())
            .unwrap();
        let check_response = router(state.clone()).oneshot(check).await.unwrap();
        assert_eq!(check_response.status(), StatusCode::OK);
        let check_body = axum::body::to_bytes(check_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let check_json: serde_json::Value = serde_json::from_slice(&check_body).unwrap();
        assert_eq!(check_json["data"]["status"], "failed");

        let disable = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/environments/{id}/disable"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap();
        let disable_response = router(state.clone()).oneshot(disable).await.unwrap();
        assert_eq!(disable_response.status(), StatusCode::OK);
        let delete = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v1/environments/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap();
        assert_eq!(
            router(state).oneshot(delete).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn container_action_api_validates_status_and_deduplicates_tasks() {
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

        let stop_request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/101/actions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "action": "shutdown",
                        "idempotency_key": "shutdown-1",
                        "confirmed": false
                    })
                    .to_string(),
                ))
                .unwrap()
        };
        let first = router(state.clone()).oneshot(stop_request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let task_id = first_json["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(first_json["data"]["status"], "queued");

        let duplicate = router(state.clone()).oneshot(stop_request()).await.unwrap();
        assert_eq!(duplicate.status(), StatusCode::OK);

        let start = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/101/actions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "action": "start",
                    "idempotency_key": "start-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap();
        assert_eq!(
            router(state.clone()).oneshot(start).await.unwrap().status(),
            StatusCode::CONFLICT
        );

        let status = router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/container-actions/{task_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn secret_api_redacts_values_and_records_lifecycle_audit() {
        let state = ApiState::new().with_auth_config(AuthConfig::disabled());
        let create = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/secrets")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "test-proxmox-token",
                    "kind": "proxmox_api_token",
                    "scope": {"type": "global"},
                    "value": "never-return-this-value"
                })
                .to_string(),
            ))
            .unwrap();
        let response = router(state.clone()).oneshot(create).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = json["data"]["metadata"]["metadata"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!json.to_string().contains("never-return-this-value"));

        let rotate_without_confirmation = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/secrets/{id}/rotate"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"value":"rotated-value","confirmed":false}).to_string(),
            ))
            .unwrap();
        assert_eq!(
            router(state.clone())
                .oneshot(rotate_without_confirmation)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );

        let revoke = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/secrets/{id}/revoke"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap();
        assert_eq!(
            router(state.clone())
                .oneshot(revoke)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let audit = router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/secrets/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(audit.status(), StatusCode::OK);
        let audit_body = axum::body::to_bytes(audit.into_body(), usize::MAX)
            .await
            .unwrap();
        let audit_json: serde_json::Value = serde_json::from_slice(&audit_body).unwrap();
        assert_eq!(audit_json["data"][0]["action"], "created");
        assert!(!audit_json.to_string().contains("never-return-this-value"));
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!(parse_container_id("0").is_err());
        assert!(parse_uuid("not-an-id", "plan id").is_err());
    }
}
