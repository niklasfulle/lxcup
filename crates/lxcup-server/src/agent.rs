use super::{
    ActorRole, AgentClient, AgentClientConfig, AgentHealth, AgentMetrics, AgentRegistration,
    ApiEnvelope, ApiError, ApiEvent, ApiState, ContainerId, Deserialize, DockerContainerInfo,
    DockerWorkload, DockerWorkloadManagementState, Extension, Json, JsonBody, Path, Permission,
    RegisteredAgent, SecretId, SecretKind, SecretScope, SecretStoreError, SecretValue, Serialize,
    State, StatusCode, StoredSecretMetadata, envelope, parse_container_id, parse_uuid,
    require_permission,
};

#[derive(Clone, Debug, Deserialize)]
pub struct ConfirmedRequest {
    pub confirmed: bool,
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

mod secrets;
pub(super) use secrets::*;
mod docker;
pub(super) use docker::*;

pub(super) async fn register_agent(
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

pub(super) fn map_secret_error(error: SecretStoreError) -> ApiError {
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

pub(super) async fn revoke_agent(
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

pub(super) async fn get_agent_health(
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

pub(super) async fn get_agent_metrics(
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

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
