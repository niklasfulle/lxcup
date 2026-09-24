use super::{ApiEnvelope, ApiError, ApiState, envelope, require_permission};
use axum::{
    Json,
    extract::{Json as JsonBody, State},
};
use lxcup_core::{ActorRole, Permission, TargetId, UpdatePolicy, UpdateRisk};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateUpdatePolicyRequest {
    pub id: String,
    pub allowed_targets: Vec<TargetId>,
    #[serde(default)]
    pub allowed_packages: Vec<String>,
    pub maintenance_start_minute: u16,
    pub maintenance_end_minute: u16,
    pub timezone: String,
    pub maximum_risk: UpdateRisk,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub(super) async fn list_update_policies(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<UpdatePolicy>>> {
    Json(envelope(state.store.read().await.update_policies.clone()))
}

pub(super) async fn create_update_policy(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateUpdatePolicyRequest>,
) -> Result<(axum::http::StatusCode, Json<ApiEnvelope<UpdatePolicy>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if request.id.trim().is_empty() || request.id.len() > 128 {
        return Err(ApiError::bad_request(
            "invalid_policy",
            "policy id is required and bounded",
        ));
    }
    if request.timezone.trim().is_empty() || request.timezone.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_timezone",
            "timezone is required and bounded",
        ));
    }
    if request.maintenance_start_minute >= 24 * 60 || request.maintenance_end_minute >= 24 * 60 {
        return Err(ApiError::bad_request(
            "invalid_window",
            "maintenance minutes must be within a day",
        ));
    }
    if request.allowed_targets.is_empty() {
        return Err(ApiError::bad_request(
            "target_required",
            "at least one target is required",
        ));
    }
    let mut store = state.store.write().await;
    if store
        .update_policies
        .iter()
        .any(|policy| policy.id == request.id)
    {
        return Err(ApiError::conflict(
            "policy_exists",
            "policy id already exists",
        ));
    }
    if request
        .allowed_targets
        .iter()
        .any(|id| !store.targets.iter().any(|target| target.id == *id))
    {
        return Err(ApiError::not_found("policy target not found"));
    }
    let policy = UpdatePolicy {
        id: request.id,
        allowed_targets: request.allowed_targets,
        allowed_packages: request.allowed_packages,
        maintenance_start_minute: request.maintenance_start_minute,
        maintenance_end_minute: request.maintenance_end_minute,
        timezone: request.timezone,
        maximum_risk: request.maximum_risk,
        enabled: request.enabled,
    };
    store.update_policies.push(policy.clone());
    Ok((axum::http::StatusCode::CREATED, Json(envelope(policy))))
}
