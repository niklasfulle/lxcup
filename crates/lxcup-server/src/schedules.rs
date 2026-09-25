use super::{ApiEnvelope, ApiError, ApiState, ApiStore, envelope, require_permission};
use axum::{
    Json,
    extract::{Json as JsonBody, State},
};
use chrono::Utc;
use lxcup_core::{ActorRole, JobSchedule, Permission, ScheduleFrequency, TargetId, ThresholdRule};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateScheduleRequest {
    pub id: String,
    pub operation: String,
    pub timezone: String,
    pub target_ids: Vec<TargetId>,
    pub every_minutes: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub threshold: Option<ThresholdRule>,
    #[serde(default)]
    pub policy_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScheduleDto {
    pub id: String,
    pub operation: String,
    pub timezone: String,
    pub target_ids: Vec<TargetId>,
    pub every_minutes: u32,
    pub enabled: bool,
    pub threshold: Option<ThresholdRule>,
    pub policy_id: Option<String>,
    pub last_run_at: Option<chrono::DateTime<Utc>>,
    pub next_run_at: chrono::DateTime<Utc>,
    pub last_error: Option<String>,
}

impl From<&JobSchedule> for ScheduleDto {
    fn from(schedule: &JobSchedule) -> Self {
        let every_minutes = match schedule.frequency {
            ScheduleFrequency::EveryMinutes(value) => value,
        };
        Self {
            id: schedule.id.clone(),
            operation: schedule.operation.clone(),
            timezone: schedule.timezone.clone(),
            target_ids: schedule.target_ids.clone(),
            every_minutes,
            enabled: schedule.enabled,
            threshold: schedule.threshold,
            policy_id: schedule.policy_id.clone(),
            last_run_at: schedule.last_run_at,
            next_run_at: schedule.next_run_at,
            last_error: schedule.last_error.clone(),
        }
    }
}

fn default_true() -> bool {
    true
}

pub(super) async fn list_schedules(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<ScheduleDto>>>, ApiError> {
    let schedules = if let Some(repositories) = state.repositories.clone() {
        repositories
            .schedules
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.schedules.clone()
    };
    Ok(Json(envelope(
        schedules.iter().map(ScheduleDto::from).collect(),
    )))
}

pub(super) async fn create_schedule(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateScheduleRequest>,
) -> Result<(axum::http::StatusCode, Json<ApiEnvelope<ScheduleDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    validate_schedule_fields(&request)?;
    let mut store = state.store.write().await;
    validate_schedule_resources(&request, &store)?;
    let schedule = JobSchedule {
        id: request.id,
        operation: request.operation,
        timezone: request.timezone,
        target_ids: request.target_ids,
        frequency: ScheduleFrequency::EveryMinutes(request.every_minutes),
        enabled: request.enabled,
        threshold: request.threshold,
        policy_id: request.policy_id,
        last_run_at: None,
        next_run_at: Utc::now() + ScheduleFrequency::EveryMinutes(request.every_minutes).interval(),
        last_error: None,
    };
    let dto = ScheduleDto::from(&schedule);
    store.schedules.push(schedule);
    let persisted = store
        .schedules
        .last()
        .cloned()
        .expect("schedule was just inserted");
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .schedules
            .save(&persisted)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok((axum::http::StatusCode::CREATED, Json(envelope(dto))))
}

fn validate_schedule_fields(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    if request.id.trim().is_empty() || request.id.len() > 128 {
        return Err(ApiError::bad_request(
            "invalid_schedule",
            "schedule id is required and bounded",
        ));
    }
    if request.timezone.trim().is_empty() || request.timezone.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_timezone",
            "timezone is required and bounded",
        ));
    }
    if request.every_minutes == 0 || request.every_minutes > 7 * 24 * 60 {
        return Err(ApiError::bad_request(
            "invalid_interval",
            "schedule interval must be between 1 minute and 7 days",
        ));
    }
    if !matches!(
        request.operation.as_str(),
        "health_check" | "collect_package_inventory" | "update_packages"
    ) {
        return Err(ApiError::bad_request(
            "invalid_operation",
            "operation is not registered for scheduling",
        ));
    }
    if request.operation == "update_packages" && request.policy_id.is_none() {
        return Err(ApiError::bad_request(
            "update_policy_required",
            "package updates require an explicit update policy",
        ));
    }
    Ok(())
}

fn validate_schedule_resources(
    request: &CreateScheduleRequest,
    store: &ApiStore,
) -> Result<(), ApiError> {
    if store
        .schedules
        .iter()
        .any(|schedule| schedule.id == request.id)
    {
        return Err(ApiError::conflict(
            "schedule_exists",
            "schedule id already exists",
        ));
    }
    if request.target_ids.is_empty() {
        return Err(ApiError::bad_request(
            "target_required",
            "at least one target is required",
        ));
    }
    if request
        .target_ids
        .iter()
        .any(|id| !store.targets.iter().any(|target| target.id == *id))
    {
        return Err(ApiError::not_found("schedule target not found"));
    }
    validate_schedule_policy_targets(request, store)
}

fn validate_schedule_policy_targets(
    request: &CreateScheduleRequest,
    store: &ApiStore,
) -> Result<(), ApiError> {
    let Some(policy_id) = request.policy_id.as_deref() else {
        return Ok(());
    };
    let policy = store
        .update_policies
        .iter()
        .find(|policy| policy.id == policy_id && policy.enabled)
        .ok_or_else(|| ApiError::not_found("update policy not found or disabled"))?;
    if request
        .target_ids
        .iter()
        .any(|target_id| !policy.allowed_targets.contains(target_id))
    {
        return Err(ApiError::bad_request(
            "update_policy_target_denied",
            "update policy does not allow every schedule target",
        ));
    }
    Ok(())
}
