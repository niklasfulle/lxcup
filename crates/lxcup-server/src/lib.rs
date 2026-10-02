//! HTTP API boundary for the lxcup web frontend and agents.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, atomic::Ordering},
};

#[cfg(test)]
use axum::http::Method;
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
    middleware,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use lxcup_agent::{
    AgentClient, AgentClientConfig, AgentHealth, AgentHeartbeat, AgentMetrics, DockerContainerInfo,
};
use lxcup_ansible::AnsibleJobCoordinator;
use lxcup_core::{
    ActorRole, AgentRegistration, Container, ContainerId, DockerWorkload,
    DockerWorkloadManagementState, Enrollment, EnrollmentId, EnrollmentState, Execution,
    ExecutionId, Permission, Scan, ScanId, SecretId, SecretKind, SecretScope, SecretValue, Target,
    TargetId, TargetKind, TargetState, TargetTransport, UpdatePlan,
};
use lxcup_execution::ExecutionCoordinator;
use lxcup_persistence::Repositories;
use lxcup_safety::{HealthCheckResult, RebootRequirement, SnapshotState};
use lxcup_secrets::{InMemorySecretStore, SecretStore, SecretStoreError, StoredSecretMetadata};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

mod targets;
pub(crate) use targets::{
    create_target, delete_target, get_target, list_targets, receive_agent_heartbeat,
    require_permission,
};
mod workflows;
pub(crate) use workflows::{
    create_ansible_job, create_enrollment, get_ansible_job, get_ansible_job_events, get_enrollment,
    list_ansible_jobs, queue_agent_reconfiguration, reconcile_ansible_job, retry_ansible_job,
};
mod worker;
pub(crate) use worker::get_worker_availability;
mod support_diagnostics;
pub(crate) use support_diagnostics::get_support_diagnostics;
mod backups;
pub(crate) use backups::{create_backup, create_scheduled_backup, download_backup, list_backups};
mod package_inventory;
pub(crate) use package_inventory::get_package_inventory;
mod telemetry;
pub(crate) use telemetry::get_target_telemetry;
mod docker_telemetry;
pub(crate) use docker_telemetry::get_target_docker_telemetry;
mod telemetry_alerts;
pub(crate) use telemetry_alerts::list_telemetry_alerts;
mod target_docker;
pub(crate) use target_docker::{
    apply_target_docker_image_update, check_target_docker_image_update, discover_target_docker,
    get_target_docker_inventory, operate_target_docker_container,
};
mod schedules;
pub(crate) use schedules::{create_schedule, list_schedules, set_schedule_enabled};
mod policies;
pub(crate) use policies::{
    STANDARD_UPDATE_POLICY_ID, create_update_policy, default_update_policy, delete_update_policy,
    list_update_policies,
};
mod inventory;
pub use inventory::AuthConfig;
pub(crate) use inventory::{
    ApiMetrics, abort_execution, auth_session, confirm_plan, create_plan, get_execution_result,
    get_plan, list_container_plans, list_containers, list_scans, live_health, logout, metrics,
    ready_health, reconcile_execution, request_middleware, run_execution, run_scan, start_scan,
};
mod user_auth;
pub(crate) use user_auth::{
    AuthenticatedUser, auth_login, auth_status, change_password, hash_session_token,
};
mod user_admin;
pub(crate) use user_admin::{
    create_user, delete_user, list_user_audit, list_users, reset_user_password, update_user,
};
mod agent;
pub(crate) use agent::{
    DockerWorkloadDto, SecretAuditEvent, adopt_docker_container, create_secret, delete_secret,
    discover_docker_containers, get_agent_health, get_agent_metrics, get_docker_discovery,
    get_secret, list_docker_containers, list_secret_audit, list_secrets, map_secret_error,
    register_agent, remove_docker_container, revoke_agent, revoke_secret, rotate_secret,
    run_docker_discovery,
};

mod errors;
pub use errors::{ApiError, ApiErrorBody, ApiErrorInfo};
mod api_types;
pub use api_types::{ApiEvent, ContainerDto, ExecutionDto, PlanDto, ScanDto};
pub(crate) use api_types::{ExecutionResultDto, truncate};

mod target_credentials;
pub(crate) use target_credentials::{
    disable_schedules_for_targets, invalidate_registered_agents, persist_changed_targets,
    reset_targets_for_secret,
};
mod scheduled_jobs;
pub(crate) use scheduled_jobs::dispatch_scheduled_target;

#[derive(Clone)]
pub struct ApiState {
    store: Arc<RwLock<ApiStore>>,
    execution: Arc<RwLock<ExecutionCoordinator>>,
    events: broadcast::Sender<ApiEvent>,
    agents: Arc<RwLock<HashMap<ContainerId, RegisteredAgent>>>,
    metrics: Arc<ApiMetrics>,
    auth: AuthConfig,
    account_auth_enabled: bool,
    secure_cookies: bool,
    repositories: Option<Repositories>,
    ansible: Arc<RwLock<AnsibleJobCoordinator>>,
    secrets: Arc<dyn SecretStore>,
    scheduler_lock: Arc<Mutex<()>>,
    docker_discovery_lock: Arc<Mutex<()>>,
    backup_gate: Arc<RwLock<()>>,
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
            account_auth_enabled: false,
            secure_cookies: std::env::var("LXCUP_ENV")
                .is_ok_and(|environment| environment.eq_ignore_ascii_case("production")),
            repositories: None,
            ansible: Arc::new(RwLock::new(AnsibleJobCoordinator::default())),
            secrets: Arc::new(InMemorySecretStore::default()),
            scheduler_lock: Arc::new(Mutex::new(())),
            docker_discovery_lock: Arc::new(Mutex::new(())),
            backup_gate: Arc::new(RwLock::new(())),
        }
    }

    pub fn with_repositories(mut self, repositories: Repositories) -> Self {
        self.repositories = Some(repositories);
        self
    }

    pub fn with_account_auth(mut self) -> Self {
        self.account_auth_enabled = true;
        self
    }

    pub async fn initialize_bootstrap_admin(&self) -> Result<bool, ApiError> {
        let repositories = self.repositories.as_ref().ok_or_else(ApiError::storage)?;
        let password_hash = tokio::task::spawn_blocking(|| user_auth::hash_password("admin"))
            .await
            .map_err(|_| ApiError::storage())?
            .map_err(|_| ApiError::storage())?;
        repositories
            .auth
            .create_initial_admin(&password_hash)
            .await
            .map_err(|_| ApiError::storage())
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
        match self.ensure_default_update_policies().await {
            Ok(created_or_extended) => restored + created_or_extended,
            Err(error) => {
                tracing::error!(error = ?error, "could not reconcile shared standard update policy");
                restored
            }
        }
    }

    /// Creates or extends the shared standard package policy to cover all
    /// registered resources. Operator-defined policy settings are preserved.
    pub async fn ensure_default_update_policies(&self) -> Result<usize, ApiError> {
        let target_ids = self.registered_target_ids().await;
        if target_ids.is_empty() {
            return Ok(0);
        }

        let existing = self
            .store
            .read()
            .await
            .update_policies
            .iter()
            .find(|policy| policy.id == STANDARD_UPDATE_POLICY_ID)
            .cloned();
        let mut policy = existing
            .clone()
            .unwrap_or_else(|| default_update_policy(target_ids.clone()));
        let before_targets = policy.allowed_targets.len();
        policy.allowed_targets.extend(target_ids);
        policy
            .allowed_targets
            .sort_by_key(|target_id| target_id.as_uuid());
        policy.allowed_targets.dedup();
        let changed = existing.is_none() || policy.allowed_targets.len() != before_targets;
        if !changed {
            return Ok(0);
        }

        let persisted = match self
            .persist_standard_update_policy(&mut policy, existing.is_some())
            .await
        {
            Ok(persisted) => persisted,
            Err(error) => {
                tracing::error!(error = ?error, "could not persist shared standard update policy");
                return Err(error);
            }
        };
        if !persisted {
            return Ok(0);
        }

        let mut store = self.store.write().await;
        if let Some(existing) = store
            .update_policies
            .iter_mut()
            .find(|existing| existing.id == STANDARD_UPDATE_POLICY_ID)
        {
            *existing = policy;
        } else {
            store.update_policies.push(policy);
        }
        Ok(1)
    }

    async fn registered_target_ids(&self) -> Vec<TargetId> {
        let mut target_ids = self
            .store
            .read()
            .await
            .targets
            .iter()
            .map(|target| target.id)
            .collect::<Vec<_>>();
        target_ids.sort_by_key(|target_id| target_id.as_uuid());
        target_ids.dedup();
        target_ids
    }

    async fn persist_standard_update_policy(
        &self,
        policy: &mut lxcup_core::UpdatePolicy,
        already_exists: bool,
    ) -> Result<bool, ApiError> {
        let Some(repositories) = self.repositories.as_ref() else {
            return Ok(true);
        };
        if already_exists {
            return repositories
                .update_policies
                .save(policy)
                .await
                .map(|()| true)
                .map_err(|_| ApiError::storage());
        }
        persist_new_standard_policy(repositories, policy).await
    }

    /// Claims due schedules and turns each target run into the same validated
    /// Ansible job contract used by the interactive API. The lock makes the
    /// poller safe when more than one tick overlaps during a slow database or
    /// worker operation.
    pub async fn dispatch_due_schedules(&self) -> usize {
        let _guard = self.scheduler_lock.lock().await;
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
            if self.schedule_waits_for_worker(&schedule).await {
                continue;
            }
            let (completed, failure) = self.execute_due_schedule(&schedule, now).await;
            dispatched += completed;
            if let Some(updated) = self
                .advance_schedule_after_run(&schedule, now, failure)
                .await
            {
                if let Some(repositories) = self.repositories.clone() {
                    let _ = repositories.schedules.update(&updated).await;
                }
            }
        }
        dispatched
    }

    async fn schedule_waits_for_worker(&self, schedule: &lxcup_core::JobSchedule) -> bool {
        !matches!(
            schedule.operation.as_str(),
            "docker_discovery" | "create_backup"
        ) && !self.scheduled_worker_available().await
    }

    async fn execute_due_schedule(
        &self,
        schedule: &lxcup_core::JobSchedule,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (usize, Option<String>) {
        if schedule.operation == "create_backup" {
            return self.execute_scheduled_backup(schedule).await;
        }
        let mut dispatched = 0;
        let mut failure = None;
        for target_id in &schedule.target_ids {
            match dispatch_scheduled_target(self, schedule, *target_id, now).await {
                Ok(true) => dispatched += 1,
                Ok(false) => {}
                Err(error) => failure = Some(error),
            }
        }
        (dispatched, failure)
    }

    async fn execute_scheduled_backup(
        &self,
        schedule: &lxcup_core::JobSchedule,
    ) -> (usize, Option<String>) {
        let result = match schedule.backup_secret_ref {
            Some(secret_ref) => create_scheduled_backup(self, secret_ref).await,
            None => Err(ApiError::bad_request(
                "backup_secret_required",
                "scheduled backup has no passphrase secret configured",
            )),
        };
        match result {
            Ok(_) => (1, None),
            Err(error) => {
                if schedule.last_error.as_deref() != Some(error.message) {
                    self.publish(ApiEvent::status("backup", schedule.id.clone(), "failed"));
                }
                (0, Some(error.message.to_owned()))
            }
        }
    }

    async fn advance_schedule_after_run(
        &self,
        schedule: &lxcup_core::JobSchedule,
        now: chrono::DateTime<chrono::Utc>,
        failure: Option<String>,
    ) -> Option<lxcup_core::JobSchedule> {
        let mut store = self.store.write().await;
        let current = store
            .schedules
            .iter_mut()
            .find(|item| item.id == schedule.id)?;
        current.advance_after_run(now, failure);
        Some(current.clone())
    }

    /// Applies configured history retention independently of request traffic.
    pub async fn prune_retained_data(&self) -> Result<(u64, u64), String> {
        let Some(repositories) = self.repositories.as_ref() else {
            return Ok((0, 0));
        };
        let telemetry = repositories
            .telemetry
            .prune_expired()
            .await
            .map_err(|error| format!("telemetry retention: {error}"))?;
        let docker_events = repositories
            .target_docker_inventory
            .prune_expired_events()
            .await
            .map_err(|error| format!("Docker event retention: {error}"))?;
        Ok((telemetry, docker_events))
    }

    async fn scheduled_worker_available(&self) -> bool {
        let Some(repositories) = self.repositories.as_ref() else {
            return true;
        };
        repositories
            .worker_heartbeats
            .latest_since(chrono::Utc::now() - chrono::Duration::seconds(10))
            .await
            .ok()
            .flatten()
            .is_some()
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
        invalidate_registered_agents(self, secret_id).await;
        let (affected_targets, changed_targets) = reset_targets_for_secret(self, secret_id).await;
        persist_changed_targets(self, changed_targets).await;
        disable_schedules_for_targets(self, affected_targets).await;
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

async fn persist_new_standard_policy(
    repositories: &Repositories,
    policy: &mut lxcup_core::UpdatePolicy,
) -> Result<bool, ApiError> {
    match repositories.update_policies.save_if_absent(policy).await {
        Ok(true) => Ok(true),
        Ok(false) => merge_concurrent_standard_policy(repositories, policy).await,
        Err(_) => Err(ApiError::storage()),
    }
}

async fn merge_concurrent_standard_policy(
    repositories: &Repositories,
    policy: &mut lxcup_core::UpdatePolicy,
) -> Result<bool, ApiError> {
    let persisted = repositories
        .update_policies
        .list()
        .await
        .map_err(|_| ApiError::storage())?;
    let Some(mut current) = persisted
        .into_iter()
        .find(|item| item.id == STANDARD_UPDATE_POLICY_ID)
    else {
        return Ok(false);
    };
    let original_count = current.allowed_targets.len();
    current
        .allowed_targets
        .extend(policy.allowed_targets.iter().copied());
    current
        .allowed_targets
        .sort_by_key(|target_id| target_id.as_uuid());
    current.allowed_targets.dedup();
    *policy = current;
    if policy.allowed_targets.len() == original_count {
        return Ok(true);
    }
    repositories
        .update_policies
        .save(policy)
        .await
        .map(|()| true)
        .map_err(|_| ApiError::storage())
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
    target_docker_inventories: HashMap<TargetId, lxcup_persistence::PersistedTargetDockerInventory>,
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
    pub target_id: Option<TargetId>,
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
            target_id: enrollment.target_id,
            state: enrollment.state,
            failure_reason: enrollment.failure_reason.clone(),
            created_at: enrollment.created_at,
            updated_at: enrollment.updated_at,
        }
    }
}

mod http_router;
pub use http_router::router;

#[derive(Clone, Debug, Serialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
    pub request_id: Uuid,
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
  "components": {"securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}, "sessionCookie": {"type": "apiKey", "in": "cookie", "name": "lxcup_session"}, "csrfToken": {"type": "apiKey", "in": "header", "name": "X-CSRF-Token"}}},
  "paths": {
    "/health/live": {"get": {}},
    "/health/ready": {"get": {}},
    "/metrics": {"get": {}},
    "/api/v1/auth/status": {"get": {"description": "Reports whether local account authentication is enabled"}},
    "/api/v1/auth/login": {"post": {"description": "Authenticates a local account and sets an eight-hour HttpOnly session cookie; the session secret is not returned in JSON"}},
    "/api/v1/auth/session": {"get": {"description": "Returns the current account role and forced-password-change state"}},
    "/api/v1/auth/logout": {"post": {"description": "Revokes the current account session and clears the session and CSRF cookies"}},
    "/api/v1/auth/password": {"post": {"description": "Changes the current account password; cookie-authenticated requests require X-CSRF-Token"}},
    "/api/v1/auth/audit": {"get": {"description": "Admin-only, filtered and paginated user activity audit history", "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer", "minimum": 1, "maximum": 100, "default": 50}}, {"name": "offset", "in": "query", "schema": {"type": "integer", "minimum": 0}}, {"name": "actor_username", "in": "query", "schema": {"type": "string"}}, {"name": "action", "in": "query", "schema": {"type": "string"}}, {"name": "resource", "in": "query", "schema": {"type": "string"}}, {"name": "since", "in": "query", "schema": {"type": "string", "format": "date-time"}}, {"name": "until", "in": "query", "schema": {"type": "string", "format": "date-time"}}], "responses": {"200": {"description": "Filtered audit page with total result count, limit, and offset"}}}},
    "/api/v1/users": {"get": {"description": "Admin-only user listing"}, "post": {"description": "Admin-only account creation without email"}},
    "/api/v1/users/{user_id}": {"patch": {"description": "Admin-only role or account-state update"}, "delete": {"description": "Admin-only confirmed user deletion"}},
    "/api/v1/users/{user_id}/password-reset": {"post": {"description": "Admin-only password reset that requires a change at next login"}},
    "/api/v1/enrollments": {"post": {"responses": {"202": {"description": "Enrollment accepted"}}}},
    "/api/v1/telemetry-alerts": {"get": {"responses": {"200": {"description": "Active telemetry threshold and freshness alerts"}}}},
    "/api/v1/targets/{target_id}/docker/telemetry": {"get": {"responses": {"200": {"description": "Recent per-container Docker CPU and memory telemetry"}}}},
    "/api/v1/targets/{target_id}": {"get": {"responses": {"200": {"description": "Registered target including the most recent authenticated agent heartbeat timestamp"}}}, "delete": {"description": "Admin-only confirmed removal of a target and its target-scoped operational data; does not uninstall the host agent or delete shared secrets", "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["confirmed"], "properties": {"confirmed": {"type": "boolean"}}}}}}, "responses": {"204": {"description": "Target removed"}, "409": {"description": "Target has active workflows"}}}},
    "/api/v1/targets/{target_id}/docker/containers/{container_id}/action": {"post": {"responses": {"200": {"description": "Confirmed allow-listed Docker lifecycle action"}}}},
    "/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check": {"post": {"responses": {"200": {"description": "Read-only registry digest comparison for a Docker image"}}}},
    "/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-apply": {"post": {"responses": {"200": {"description": "Explicitly confirmed Compose image update for one Linux service"}}}},
    "/api/v1/enrollments/{enrollment_id}": {"get": {"responses": {"200": {"description": "Enrollment status"}}}},
    "/api/v1/ansible/jobs": {"post": {"responses": {"202": {"description": "Ansible job accepted"}}}},
    "/api/v1/ansible/jobs/{job_id}": {"get": {"responses": {"200": {"description": "Ansible job status"}}}},
    "/api/v1/ansible/jobs/{job_id}/events": {"get": {"responses": {"200": {"description": "Audit-safe Ansible job events"}}}},
    "/api/v1/admin/support-diagnostics": {"get": {"description": "Admin-only, bounded local support diagnostics; excludes secrets and raw worker output", "parameters": [{"name": "days", "in": "query", "schema": {"type": "integer", "minimum": 1, "maximum": 30, "default": 7}}], "responses": {"200": {"description": "Locally downloadable support diagnostics"}, "403": {"description": "Admin role required"}}}},
    "/api/v1/admin/backups": {"get": {"description": "Admin-only list of retained encrypted database and secret-store backups"}, "post": {"description": "Create an explicitly confirmed age passphrase-encrypted backup, retain it in the configured backup volume, and return its metadata", "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["confirmed", "passphrase"], "properties": {"confirmed": {"type": "boolean"}, "passphrase": {"type": "string", "minLength": 12, "maxLength": 1024, "description": "Human-selected age encryption passphrase; never stored"}}}}}}, "responses": {"200": {"description": "Encrypted backup created"}, "400": {"description": "Confirmation or passphrase is invalid"}, "403": {"description": "Admin role required"}, "502": {"description": "Backup tools or configuration unavailable"}}}},
    "/api/v1/admin/backups/{backup_id}": {"get": {"description": "Admin-only streaming download of a retained encrypted backup"}},
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
    "/api/v1/containers/{container_id}/docker/discover": {"post": {"responses": {"200": {"description": "Discover Docker workloads"}}}},
    "/api/v1/targets/{target_id}/docker/discovery": {"get": {"responses": {"200": {"description": "Latest successful Docker inventory for target"}}}, "post": {"responses": {"200": {"description": "Discover and persist Docker workloads"}}}},
    "/api/v1/schedules": {"get": {"description": "List configured recurring schedules; secret values and secret references are omitted"}, "post": {"description": "Create a recurring registered operation. Admin-only create_backup schedules require an active global backup-passphrase Secret reference, run hourly to weekly in UTC, and emit status-only activity events", "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["id", "operation", "timezone", "target_ids", "every_minutes"], "properties": {"id": {"type": "string", "maxLength": 128}, "operation": {"type": "string", "enum": ["health_check", "collect_package_inventory", "update_packages", "docker_discovery", "create_backup"]}, "timezone": {"type": "string"}, "target_ids": {"type": "array", "items": {"type": "string", "format": "uuid"}}, "every_minutes": {"type": "integer", "minimum": 1, "maximum": 10080}, "backup_secret_ref": {"type": "string", "format": "uuid", "description": "Required for create_backup; secret value is never returned"}}}}}}, "responses": {"201": {"description": "Schedule created"}, "403": {"description": "Admin role required for backup schedules"}}}},
    "/api/v1/schedules/{schedule_id}": {"patch": {"description": "Enable or pause a recurring schedule"}},
    "/api/v1/update-policies": {"get": {}, "post": {}},
    "/api/v1/update-policies/{policy_id}": {"delete": {"description": "Delete a confirmed non-system update policy"}},
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
    axum::Extension(actor_role): axum::Extension<ActorRole>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = BroadcastStream::new(receiver).filter_map(move |result| match result {
        Ok(event) if event_visible_to_role(&event, actor_role) => Some(Ok(Event::default()
            .event(event.kind())
            .json_data(&event)
            .unwrap_or_else(|_| Event::default()))),
        Ok(_) => None,
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

fn event_visible_to_role(event: &ApiEvent, role: ActorRole) -> bool {
    role == ActorRole::Admin
        || !matches!(event, ApiEvent::Status { resource, .. } if resource == "backup")
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

#[cfg(test)]
mod tests;
