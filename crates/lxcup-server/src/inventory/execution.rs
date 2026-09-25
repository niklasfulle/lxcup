use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, ExecutionDto, ExecutionResultDto, PlanDto, envelope,
    parse_container_id, parse_uuid, truncate,
};
use axum::{
    Json,
    extract::{Json as JsonBody, Path, State},
    http::{HeaderMap, StatusCode},
};
use lxcup_agent::{AgentAction, AgentCommandRequest};
use lxcup_core::PlanStatus;
use lxcup_execution::{ExecutionRequest, ExecutionStart};
use lxcup_planner::{DryRunChange, PlannerInput, UpdatePlanner};
use serde::Deserialize;

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

pub(crate) fn default_authenticated() -> bool {
    true
}

pub(crate) async fn list_container_plans(
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

pub(crate) async fn create_plan(
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

pub(crate) async fn get_plan(
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

pub(crate) async fn confirm_plan(
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

pub(crate) async fn abort_execution(
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

pub(crate) async fn run_execution(
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

pub(crate) async fn reconcile_execution(
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

pub(crate) async fn get_execution_result(
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
