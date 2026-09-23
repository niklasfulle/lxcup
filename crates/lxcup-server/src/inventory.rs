use super::*;

pub(super) async fn list_node_containers(
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

pub(super) async fn list_containers(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<ContainerDto>>> {
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

pub(super) async fn list_scans(
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

pub(super) async fn start_scan(
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

pub(crate) struct ApiMetrics {
    pub(crate) started_at: Option<Instant>,
    pub(crate) requests_total: AtomicU64,
    pub(crate) requests_failed: AtomicU64,
    pub(crate) events_published: AtomicU64,
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

pub(super) async fn request_middleware(
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

pub(super) async fn live_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub(super) async fn ready_health(State(state): State<ApiState>) -> impl IntoResponse {
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

pub(super) async fn metrics(State(state): State<ApiState>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        state.metrics.render(),
    )
}

pub(super) async fn run_scan(
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

pub(super) fn default_authenticated() -> bool {
    true
}

pub(super) async fn list_container_plans(
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

pub(super) async fn create_plan(
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

pub(super) async fn get_plan(
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

pub(super) async fn confirm_plan(
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

pub(super) async fn abort_execution(
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

pub(super) async fn run_execution(
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

pub(super) async fn reconcile_execution(
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

pub(super) async fn get_execution_result(
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
