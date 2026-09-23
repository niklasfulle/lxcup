use super::*;

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

pub(super) async fn list_secrets(
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

pub(super) async fn create_secret(
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

pub(super) async fn list_secret_audit(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<SecretAuditEvent>>> {
    Json(envelope(state.store.read().await.secret_audit.clone()))
}

pub(super) async fn get_secret(
    State(state): State<ApiState>,
    Path(secret_id): Path<String>,
) -> Result<Json<ApiEnvelope<SecretMetadataDto>>, ApiError> {
    let id = parse_secret_id(&secret_id)?;
    let metadata = state.secrets.metadata(id).map_err(map_secret_error)?;
    Ok(Json(envelope(SecretMetadataDto { metadata })))
}

pub(super) async fn rotate_secret(
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

pub(super) async fn revoke_secret(
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

pub(super) async fn delete_secret(
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

pub(super) async fn record_secret_audit(
    state: &ApiState,
    id: SecretId,
    action: &str,
    role: ActorRole,
) {
    let event = SecretAuditEvent {
        secret_id: id,
        action: action.to_owned(),
        role,
        occurred_at: chrono::Utc::now(),
    };
    state.store.write().await.secret_audit.push(event);
    state.publish(ApiEvent::status("secret", id.as_uuid().to_string(), action));
}

pub(super) fn parse_secret_id(value: &str) -> Result<SecretId, ApiError> {
    Ok(SecretId::from_uuid(parse_uuid(value, "secret id")?))
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

pub(super) async fn list_docker_containers(
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

pub(super) async fn discover_docker_containers(
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

pub(super) async fn adopt_docker_container(
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
pub(crate) struct RemoveDockerWorkloadRequest {
    confirmed: bool,
}

pub(super) async fn remove_docker_container(
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
