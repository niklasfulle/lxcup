//! HTTP API boundary for the lxcup web frontend and agents.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, atomic::Ordering},
};

#[cfg(test)]
use axum::http::Method;
use axum::{
    Json, Router,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
    middleware,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use lxcup_agent::{
    AgentClient, AgentClientConfig, AgentHealth, AgentHeartbeat, AgentMetrics, DockerContainerInfo,
};
use lxcup_ansible::{
    AnsibleJobCoordinator, AnsibleJobRequest, AnsibleOperation, AnsibleParameters, ExecutionMode,
    JobSubmission,
};
use lxcup_core::{
    ActorRole, AgentRegistration, Container, ContainerId, DockerWorkload,
    DockerWorkloadManagementState, Enrollment, EnrollmentId, EnrollmentState, Execution,
    ExecutionId, Permission, ResourceLifecycle, ResourceTarget, Scan, ScanId, SecretId, SecretKind,
    SecretScope, SecretValue, Target, TargetId, TargetKind, TargetState, TargetTransport,
    ThresholdMetric, ThresholdRule, UpdatePlan, UpdatePlanId, UpdateRisk,
};
use lxcup_execution::ExecutionCoordinator;
use lxcup_persistence::Repositories;
use lxcup_safety::{HealthCheckResult, RebootRequirement, SnapshotState};
use lxcup_secrets::{
    CreateSecret, InMemorySecretStore, SecretStore, SecretStoreError, StoredSecretMetadata,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

mod targets;
pub(crate) use targets::{
    create_target, get_target, list_targets, receive_agent_heartbeat, require_permission,
};
mod workflows;
pub(crate) use workflows::{
    create_ansible_job, create_enrollment, get_ansible_job, get_ansible_job_events, get_enrollment,
    list_ansible_jobs, queue_agent_reconfiguration, retry_ansible_job,
};
mod worker;
pub(crate) use worker::get_worker_availability;
mod package_inventory;
pub(crate) use package_inventory::get_package_inventory;
mod telemetry;
pub(crate) use telemetry::get_target_telemetry;
mod schedules;
pub(crate) use schedules::{create_schedule, list_schedules};
mod policies;
pub(crate) use policies::{create_update_policy, list_update_policies};
mod inventory;
pub use inventory::AuthConfig;
pub(crate) use inventory::{
    ApiMetrics, abort_execution, auth_session, confirm_plan, create_plan, get_execution_result,
    get_plan, list_container_plans, list_containers, list_scans, live_health, logout, metrics,
    ready_health, reconcile_execution, request_middleware, run_execution, run_scan, start_scan,
};
mod agent;
pub(crate) use agent::{
    DockerWorkloadDto, SecretAuditEvent, adopt_docker_container, create_secret, delete_secret,
    discover_docker_containers, get_agent_health, get_agent_metrics, get_docker_discovery,
    get_secret, list_docker_containers, list_secret_audit, list_secrets, map_secret_error,
    register_agent, remove_docker_container, revoke_agent, revoke_secret, rotate_secret,
};

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
    scheduler_lock: Arc<Mutex<()>>,
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
            scheduler_lock: Arc::new(Mutex::new(())),
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

    /// Restores persisted schedules so a restart cannot silently disable
    /// recurring maintenance jobs.
    pub async fn restore_schedules(&self) -> usize {
        let Some(repositories) = self.repositories.as_ref() else {
            return 0;
        };
        let Ok(schedules) = repositories.schedules.list().await else {
            return 0;
        };
        let restored = schedules.len();
        self.store.write().await.schedules = schedules;
        restored
    }

    pub async fn restore_update_policies(&self) -> usize {
        let Some(repositories) = self.repositories.as_ref() else {
            return 0;
        };
        let Ok(policies) = repositories.update_policies.list().await else {
            return 0;
        };
        let restored = policies.len();
        self.store.write().await.update_policies = policies;
        restored
    }

    /// Claims due schedules and turns each target run into the same validated
    /// Ansible job contract used by the interactive API. The lock makes the
    /// poller safe when more than one tick overlaps during a slow database or
    /// worker operation.
    pub async fn dispatch_due_schedules(&self) -> usize {
        let _guard = self.scheduler_lock.lock().await;
        if let Some(repositories) = self.repositories.clone() {
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(10);
            let worker_available = repositories
                .worker_heartbeats
                .latest_since(cutoff)
                .await
                .ok()
                .flatten()
                .is_some();
            if !worker_available {
                return 0;
            }
        }
        let now = chrono::Utc::now();
        let due = self
            .store
            .read()
            .await
            .schedules
            .iter()
            .filter(|schedule| schedule.is_due(now))
            .cloned()
            .collect::<Vec<_>>();
        let mut dispatched = 0;
        for schedule in due {
            let mut failure = None;
            for target_id in &schedule.target_ids {
                let target = self
                    .store
                    .read()
                    .await
                    .targets
                    .iter()
                    .find(|target| target.id == *target_id && target.state == TargetState::Managed)
                    .cloned();
                let Some(target) = target else {
                    failure = Some(format!("target {} is not managed", target_id.as_uuid()));
                    continue;
                };
                if let Some(repositories) = self.repositories.clone() {
                    if repositories
                        .ansible_jobs
                        .has_active_target(ResourceTarget::Target(*target_id))
                        .await
                        .unwrap_or(false)
                    {
                        failure = Some(format!(
                            "target {} already has an active job",
                            target_id.as_uuid()
                        ));
                        continue;
                    }
                }
                if let Some(rule) = schedule.threshold {
                    let actual = self
                        .store
                        .read()
                        .await
                        .agent_reports
                        .get(target_id)
                        .and_then(|heartbeat| threshold_value(rule, heartbeat));
                    if !rule.triggered(actual) {
                        let message = format!(
                            "schedule {} threshold not met for target {}",
                            schedule.id,
                            target_id.as_uuid()
                        );
                        failure = Some(message.clone());
                        self.publish(ApiEvent::Error {
                            code: "schedule_threshold_not_met".to_owned(),
                            message,
                            request_id: schedule.id.clone(),
                        });
                        continue;
                    }
                }
                let update_policy = if schedule.operation == "update_packages" {
                    if let Some(policy_id) = schedule.policy_id.as_deref() {
                        self.store
                            .read()
                            .await
                            .update_policies
                            .iter()
                            .find(|policy| policy.id == policy_id)
                            .cloned()
                    } else {
                        None
                    }
                } else {
                    None
                };
                let (operation, parameters, mode, secret_refs) = match schedule.operation.as_str() {
                    "health_check" => (
                        AnsibleOperation::HealthCheck,
                        AnsibleParameters::HealthCheck,
                        ExecutionMode::Check,
                        Vec::new(),
                    ),
                    "collect_package_inventory" => (
                        AnsibleOperation::CollectPackageInventory,
                        AnsibleParameters::CollectPackageInventory,
                        ExecutionMode::Check,
                        vec![target.credential_secret_ref],
                    ),
                    "update_packages" => {
                        let Some(policy) = update_policy else {
                            failure =
                                Some("scheduled package update has no valid policy".to_owned());
                            continue;
                        };
                        if policy.allowed_packages.is_empty() {
                            failure = Some(
                                "scheduled package updates require an explicit package allowlist"
                                    .to_owned(),
                            );
                            continue;
                        }
                        let packages = policy.allowed_packages.clone();
                        if !policy.enabled
                            || !policy.allowed_targets.contains(target_id)
                            || packages.iter().any(|package| {
                                !policy.allows(*target_id, package, UpdateRisk::High, now)
                            })
                        {
                            failure =
                                Some("scheduled package update is outside its policy".to_owned());
                            continue;
                        }
                        (
                            AnsibleOperation::UpdatePackages,
                            AnsibleParameters::UpdatePackages { packages },
                            ExecutionMode::Plan,
                            vec![target.credential_secret_ref],
                        )
                    }
                    _ => {
                        failure = Some("schedule operation is no longer registered".to_owned());
                        continue;
                    }
                };
                let idempotency_key = format!(
                    "schedule-{}-{}-{}",
                    schedule.id,
                    target_id.as_uuid(),
                    schedule.next_run_at.timestamp()
                );
                let submission = self.ansible.write().await.submit(AnsibleJobRequest {
                    operation,
                    target: ResourceTarget::Target(*target_id),
                    lifecycle: ResourceLifecycle::Managed,
                    mode,
                    parameters,
                    secret_refs,
                    idempotency_key,
                    confirmed: true,
                    actor_role: ActorRole::Operator,
                });
                match submission {
                    Ok(JobSubmission::Created(job)) => {
                        if let Err(_error) = workflows::persist_created_job(self, &job).await {
                            failure = Some("scheduled job persistence failed".to_owned());
                        } else {
                            self.publish(ApiEvent::status(
                                "ansible_job",
                                job.id.as_uuid().to_string(),
                                "queued",
                            ));
                            dispatched += 1;
                        }
                    }
                    Ok(JobSubmission::Duplicate(_)) => {}
                    Err(error) => failure = Some(error.to_string()),
                }
            }
            let updated = {
                let mut store = self.store.write().await;
                let Some(current) = store
                    .schedules
                    .iter_mut()
                    .find(|item| item.id == schedule.id)
                else {
                    continue;
                };
                current.advance_after_run(now, failure);
                current.clone()
            };
            if let Some(repositories) = self.repositories.clone() {
                let _ = repositories.schedules.update(&updated).await;
            }
        }
        dispatched
    }

    /// Resumes the dependent jobs for completed onboarding deployments.
    /// This controller-owned reconciliation is independent of UI polling.
    pub async fn reconcile_onboarding_jobs(&self) -> usize {
        let _guard = self.scheduler_lock.lock().await;
        match workflows::reconcile_onboarding_jobs(self).await {
            Ok(queued) => queued,
            Err(error) => {
                tracing::warn!(error = ?error, "onboarding job reconciliation failed");
                0
            }
        }
    }

    #[cfg(test)]
    pub async fn replace_nodes(&self, _nodes: Vec<lxcup_core::Node>) {}

    #[cfg(test)]
    pub async fn replace_containers(&self, containers: Vec<Container>) {
        self.store.write().await.containers = containers;
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
        if let Some(repositories) = self.repositories.as_ref() {
            if let Ok(registrations) = repositories.agent_registrations.list().await {
                for registration in registrations.into_iter().filter(|item| {
                    item.secret_ref == secret_id || item.ca_secret_ref == Some(secret_id)
                }) {
                    self.agents.write().await.remove(&registration.container_id);
                    let mut registration = registration;
                    registration.record_unreachable(
                        chrono::Utc::now(),
                        "agent credentials changed; re-register or redeploy the agent",
                    );
                    let _ = repositories.agent_registrations.save(&registration).await;
                    self.publish(ApiEvent::status(
                        "agent",
                        registration.container_id.value().to_string(),
                        "credential_invalidated",
                    ));
                }
            }
        }
        let (affected_targets, changed_targets) = {
            let mut store = self.store.write().await;
            let mut affected = Vec::new();
            let mut changed = Vec::new();
            for target in &mut store.targets {
                if target.credential_secret_ref == secret_id
                    || target.agent_secret_ref == secret_id
                    || target.ssh_known_hosts_secret_ref == Some(secret_id)
                {
                    affected.push(target.id);
                    if target.agent_secret_ref == secret_id && target.state != TargetState::Disabled
                    {
                        target.state = TargetState::Pending;
                        target.updated_at = chrono::Utc::now();
                        changed.push(target.clone());
                    }
                }
            }
            (affected, changed)
        };
        if let Some(repositories) = self.repositories.as_ref() {
            for target in changed_targets {
                let _ = repositories.targets.update(&target).await;
            }
        }
        let disabled = {
            let mut store = self.store.write().await;
            let mut changed = Vec::new();
            for schedule in &mut store.schedules {
                if schedule
                    .target_ids
                    .iter()
                    .any(|target_id| affected_targets.contains(target_id))
                    && schedule.enabled
                {
                    schedule.enabled = false;
                    schedule.last_error = Some(
                        "disabled because a referenced secret was revoked or rotated".to_owned(),
                    );
                    changed.push(schedule.clone());
                }
            }
            changed
        };
        for schedule in disabled {
            if let Some(repositories) = self.repositories.clone() {
                let _ = repositories.schedules.update(&schedule).await;
            }
            self.publish(ApiEvent::status(
                "schedule",
                schedule.id,
                "disabled_secret_invalidated",
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

fn threshold_value(rule: ThresholdRule, heartbeat: &AgentHeartbeat) -> Option<u64> {
    let sample = heartbeat.telemetry.samples.last();
    match rule.metric {
        ThresholdMetric::CpuBasisPoints => {
            sample.and_then(|item| item.cpu_basis_points.map(u64::from))
        }
        ThresholdMetric::MemoryBasisPoints => {
            sample.and_then(|item| item.memory_basis_points.map(u64::from))
        }
        ThresholdMetric::StorageBasisPoints => {
            sample.and_then(|item| item.storage_basis_points.map(u64::from))
        }
        ThresholdMetric::HeartbeatAgeSeconds => Some(
            chrono::Utc::now()
                .signed_duration_since(heartbeat.sent_at)
                .num_seconds()
                .max(0) as u64,
        ),
    }
}

#[derive(Default)]
struct ApiStore {
    targets: Vec<Target>,
    agent_reports: HashMap<TargetId, AgentHeartbeat>,
    containers: Vec<Container>,
    enrollments: Vec<Enrollment>,
    enrollment_keys: HashMap<String, EnrollmentId>,
    scans: Vec<Scan>,
    plans: Vec<UpdatePlan>,
    executions: Vec<Execution>,
    safety: HashMap<ExecutionId, SafetyDto>,
    results: HashMap<ExecutionId, ExecutionResultDto>,
    secret_audit: Vec<SecretAuditEvent>,
    docker_workloads: HashMap<(ContainerId, String), DockerWorkloadDto>,
    docker_discovery_runs: Vec<lxcup_core::DockerDiscoveryRun>,
    schedules: Vec<lxcup_core::JobSchedule>,
    update_policies: Vec<lxcup_core::UpdatePolicy>,
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
        .route("/api/v1/auth/session", get(auth_session))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/targets", get(list_targets).post(create_target))
        .route(
            "/api/v1/targets/{target_id}/package-inventory",
            get(get_package_inventory),
        )
        .route(
            "/api/v1/targets/{target_id}/telemetry",
            get(get_target_telemetry),
        )
        .route("/api/v1/targets/{target_id}", get(get_target))
        .route(
            "/api/v1/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/update-policies",
            get(list_update_policies).post(create_update_policy),
        )
        .route("/api/v1/agents/heartbeat", post(receive_agent_heartbeat))
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
            "/api/v1/ansible/jobs/{job_id}/retry",
            post(retry_ansible_job),
        )
        .route(
            "/api/v1/ansible/jobs/{job_id}/events",
            get(get_ansible_job_events),
        )
        .route(
            "/api/v1/ansible/worker-availability",
            get(get_worker_availability),
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
        .route(
            "/api/v1/containers/{container_id}/docker/containers",
            get(list_docker_containers),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/discovery",
            get(get_docker_discovery),
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
    "/api/v1/containers/{container_id}/docker/discovery": {"get": {"responses": {"200": {"description": "Latest Docker discovery workflow"}}}},
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

fn parse_container_id(value: &str) -> Result<ContainerId, ApiError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(ContainerId::new)
        .ok_or_else(|| ApiError::bad_request("invalid_id", "container id"))
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerDto {
    pub id: ContainerId,
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
mod tests;
