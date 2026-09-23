use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, Permission, SecretId, Target, TargetId,
    TargetKind, TargetState, TargetTransport, envelope, map_secret_error, parse_uuid,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::{HeaderMap, StatusCode},
};
use lxcup_agent::AgentHeartbeat;
use lxcup_core::ActorRole;
use serde::{Deserialize, Serialize};

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

pub(super) async fn list_targets(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<TargetDto>>> {
    Json(envelope(
        state.store.read().await.targets.iter().map(TargetDto::from).collect(),
    ))
}

pub(super) async fn create_target(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateTargetRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<TargetDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let mut target = Target::new(request.name, request.kind, request.address, request.transport, request.credential_secret_ref, request.agent_secret_ref)
        .map_err(|_| ApiError::bad_request("invalid_target", "target fields or transport are invalid"))?;
    target.ssh_user = request.ssh_user.filter(|value| !value.trim().is_empty());
    target.ssh_known_hosts_secret_ref = request.ssh_known_hosts_secret_ref;
    let dto = TargetDto::from(&target);
    if let Some(repositories) = state.repositories.clone() {
        repositories.targets.save(&target).await.map_err(|_| ApiError::storage())?;
    }
    state.store.write().await.targets.push(target);
    state.publish(ApiEvent::status("target", dto.id.as_uuid().to_string(), "pending"));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

pub(super) async fn get_target(State(state): State<ApiState>, Path(target_id): Path<String>) -> Result<Json<ApiEnvelope<TargetDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let store = state.store.read().await;
    let target = store.targets.iter().find(|target| target.id == target_id).ok_or_else(|| ApiError::not_found("target not found"))?;
    Ok(Json(envelope(TargetDto::from(target))))
}

pub(super) async fn receive_agent_heartbeat(State(state): State<ApiState>, headers: HeaderMap, JsonBody(heartbeat): JsonBody<AgentHeartbeat>) -> Result<StatusCode, ApiError> {
    let token = headers.get("authorization").and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer ")).ok_or_else(ApiError::unauthorized)?;
    let mut store = state.store.write().await;
    let target = store.targets.iter_mut().find(|target| target.id.as_uuid() == heartbeat.target_id).ok_or_else(|| ApiError::not_found("target not found"))?;
    let expected = state.secrets.read(target.agent_secret_ref).map_err(map_secret_error)?;
    if expected.expose() != token { return Err(ApiError::unauthorized()); }
    target.mark_managed();
    let persisted = target.clone();
    store.agent_reports.insert(persisted.id, heartbeat);
    drop(store);
    if let Some(repositories) = state.repositories.clone() { repositories.targets.update(&persisted).await.map_err(|_| ApiError::storage())?; }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn require_permission(role: ActorRole, permission: Permission) -> Result<(), ApiError> {
    if role.grants(permission) { Ok(()) } else { Err(ApiError::forbidden("permission_denied", "the role cannot change this resource")) }
}
