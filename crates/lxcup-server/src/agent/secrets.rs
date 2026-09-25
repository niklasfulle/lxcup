use super::{
    ActorRole, ApiEnvelope, ApiError, ApiEvent, ApiState, SecretId, SecretKind, SecretScope,
    SecretValue, StoredSecretMetadata, envelope, map_secret_error, parse_uuid, require_permission,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
};
use lxcup_core::Permission;
use lxcup_secrets::CreateSecret;
use serde::{Deserialize, Serialize};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_job_id: Option<lxcup_core::AnsibleJobId>,
}

pub(crate) async fn list_secrets(
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

pub(crate) async fn create_secret(
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
    record_secret_audit(&state, id, "created", actor_role, None).await?;
    Ok((
        StatusCode::CREATED,
        Json(envelope(SecretMetadataDto { metadata })),
    ))
}

pub(crate) async fn list_secret_audit(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<SecretAuditEvent>>>, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        let events = repositories
            .audit_events
            .list_secret_events()
            .await
            .map_err(|_| ApiError::storage())?;
        let events = events
            .into_iter()
            .filter_map(|event| {
                let secret_id = uuid::Uuid::parse_str(event.details.get("secret_id")?.as_str()?)
                    .ok()
                    .map(SecretId::from_uuid)?;
                let role = match event.details.get("role")?.as_str()? {
                    "admin" => ActorRole::Admin,
                    "operator" => ActorRole::Operator,
                    _ => ActorRole::Viewer,
                };
                let related_job_id = event
                    .details
                    .get("related_job_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .map(lxcup_core::AnsibleJobId::from_uuid);
                Some(SecretAuditEvent {
                    secret_id,
                    action: event
                        .event_type
                        .strip_prefix("secret.")
                        .unwrap_or(&event.event_type)
                        .to_owned(),
                    role,
                    occurred_at: event.created_at,
                    related_job_id,
                })
            })
            .collect();
        return Ok(Json(envelope(events)));
    }
    Ok(Json(envelope(
        state.store.read().await.secret_audit.clone(),
    )))
}

pub(crate) async fn get_secret(
    State(state): State<ApiState>,
    Path(secret_id): Path<String>,
) -> Result<Json<ApiEnvelope<SecretMetadataDto>>, ApiError> {
    let id = parse_secret_id(&secret_id)?;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

pub(crate) async fn rotate_secret(
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
    let metadata_before = state.secrets.metadata(id).map_err(map_secret_error)?;
    state.secrets.rotate(id, value).map_err(map_secret_error)?;
    state.invalidate_agents_for_secret(id).await;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    record_secret_audit(&state, id, "rotated", actor_role, None).await?;
    if metadata_before.metadata.kind == SecretKind::AgentToken {
        let targets = state
            .store
            .read()
            .await
            .targets
            .iter()
            .filter(|target| {
                target.agent_secret_ref == id && target.state != lxcup_core::TargetState::Disabled
            })
            .cloned()
            .collect::<Vec<_>>();
        for target in targets {
            let key = format!("agent-token-reconfiguration-{}", uuid::Uuid::new_v4());
            match crate::queue_agent_reconfiguration(&state, &target, key).await {
                Ok(Some(job)) => {
                    record_secret_audit(
                        &state,
                        id,
                        "agent_reconfiguration_queued",
                        actor_role,
                        Some(job.id),
                    )
                    .await?
                }
                Ok(None) => {
                    record_secret_audit(
                        &state,
                        id,
                        "agent_reconfiguration_skipped",
                        actor_role,
                        None,
                    )
                    .await?
                }
                Err(_) => {
                    record_secret_audit(
                        &state,
                        id,
                        "agent_reconfiguration_failed",
                        actor_role,
                        None,
                    )
                    .await?
                }
            }
        }
    }
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

pub(crate) async fn revoke_secret(
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
    record_secret_audit(&state, id, "revoked", actor_role, None).await?;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

pub(crate) async fn delete_secret(
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
    state.invalidate_agents_for_secret(id).await;
    record_secret_audit(&state, id, "deleted", actor_role, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn record_secret_audit(
    state: &ApiState,
    id: SecretId,
    action: &str,
    role: ActorRole,
    related_job_id: Option<lxcup_core::AnsibleJobId>,
) -> Result<(), ApiError> {
    let occurred_at = chrono::Utc::now();
    if let Some(repositories) = state.repositories.as_ref() {
        let details = serde_json::json!({ "secret_id": id.as_uuid(), "role": format!("{role:?}").to_lowercase(), "related_job_id": related_job_id.map(|job| job.as_uuid()) });
        repositories
            .audit_events
            .append(&lxcup_persistence::AuditEvent {
                id: uuid::Uuid::new_v4(),
                node_id: None,
                container_id: None,
                plan_id: None,
                execution_id: None,
                event_type: format!("secret.{action}"),
                details,
                created_at: occurred_at,
            })
            .await
            .map_err(|_| ApiError::storage())?;
    }
    let event = SecretAuditEvent {
        secret_id: id,
        action: action.to_owned(),
        role,
        occurred_at,
        related_job_id,
    };
    state.store.write().await.secret_audit.push(event);
    state.publish(ApiEvent::status("secret", id.as_uuid().to_string(), action));
    Ok(())
}

pub(crate) fn parse_secret_id(value: &str) -> Result<SecretId, ApiError> {
    Ok(SecretId::from_uuid(parse_uuid(value, "secret id")?))
}
