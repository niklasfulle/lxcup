use super::{
    ActorRole, AgentClient, AgentClientConfig, AgentHealth, AgentMetrics, AgentRegistration,
    ApiEnvelope, ApiError, ApiEvent, ApiState, ContainerId, CreateSecret, Deserialize,
    DockerContainerInfo, DockerWorkload, DockerWorkloadManagementState, Extension, Json, JsonBody,
    Path, Permission, RegisteredAgent, SecretId, SecretKind, SecretScope, SecretStoreError,
    SecretValue, Serialize, State, StatusCode, StoredSecretMetadata, envelope, parse_container_id,
    parse_uuid, require_permission,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_job_id: Option<lxcup_core::AnsibleJobId>,
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
    record_secret_audit(&state, id, "created", actor_role, None).await?;
    Ok((
        StatusCode::CREATED,
        Json(envelope(SecretMetadataDto { metadata })),
    ))
}

pub(super) async fn list_secret_audit(
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
            match super::queue_agent_reconfiguration(&state, &target, key).await {
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
    record_secret_audit(&state, id, "revoked", actor_role, None).await?;
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
    state.invalidate_agents_for_secret(id).await;
    record_secret_audit(&state, id, "deleted", actor_role, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn record_secret_audit(
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
    pub ports: Vec<String>,
    pub started_at: Option<String>,
    pub labels: Vec<String>,
    pub presence: String,
    pub change_state: String,
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
            ports: container.ports,
            started_at: container.started_at,
            labels: container.labels,
            presence: "present".to_owned(),
            change_state: "new".to_owned(),
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
            ports: item.ports,
            started_at: item.started_at,
            labels: item.labels,
            presence: match item.presence {
                lxcup_core::DockerWorkloadPresence::Present => "present".to_owned(),
                lxcup_core::DockerWorkloadPresence::Missing => "missing".to_owned(),
            },
            change_state: match item.change {
                lxcup_core::DockerWorkloadChange::New => "new".to_owned(),
                lxcup_core::DockerWorkloadChange::Changed => "changed".to_owned(),
                lxcup_core::DockerWorkloadChange::Unchanged => "unchanged".to_owned(),
                lxcup_core::DockerWorkloadChange::Missing => "missing".to_owned(),
            },
            management_state: match item.management_state {
                DockerWorkloadManagementState::Discovered => "discovered".to_owned(),
                DockerWorkloadManagementState::Managed => "managed".to_owned(),
            },
            discovered_at: item.discovered_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DockerDiscoveryResultDto {
    pub run: lxcup_core::DockerDiscoveryRun,
    pub workloads: Vec<DockerWorkloadDto>,
}

pub(super) async fn get_docker_discovery(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Option<lxcup_core::DockerDiscoveryRun>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let latest = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .docker_discovery
            .latest(container_id)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state
            .store
            .read()
            .await
            .docker_discovery_runs
            .iter()
            .rev()
            .find(|run| run.host_container_id == container_id)
            .cloned()
    };
    Ok(Json(envelope(latest)))
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
) -> Result<Json<ApiEnvelope<DockerDiscoveryResultDto>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let mut run = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .docker_discovery
            .start(container_id)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        let run = lxcup_core::DockerDiscoveryRun {
            id: uuid::Uuid::new_v4(),
            host_container_id: container_id,
            status: lxcup_core::DockerDiscoveryStatus::Running,
            started_at: chrono::Utc::now(),
            finished_at: None,
            container_count: 0,
            error_code: None,
        };
        state
            .store
            .write()
            .await
            .docker_discovery_runs
            .push(run.clone());
        run
    };
    state.publish(ApiEvent::status(
        "docker_discovery",
        container_id.value().to_string(),
        "running",
    ));

    let agent = state
        .agents
        .read()
        .await
        .get(&container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("agent not registered"));
    let agent = match agent {
        Ok(agent) => agent,
        Err(error) => {
            finish_docker_discovery(&state, &mut run, 0, Some("agent_unavailable")).await?;
            return Err(error);
        }
    };
    let discovered = match agent.client.docker_containers().await {
        Ok(discovered) => discovered,
        Err(_) => {
            finish_docker_discovery(&state, &mut run, 0, Some("docker_discovery_failed")).await?;
            return Err(ApiError::dependency(
                "docker_discovery_failed",
                "Docker-Inventar konnte vom Agenten nicht gelesen werden",
            ));
        }
    };
    if !discovered.available {
        finish_docker_discovery(&state, &mut run, 0, Some("docker_unavailable")).await?;
        return Err(ApiError::dependency(
            "docker_unavailable",
            "Auf diesem Host ist Docker nicht verfügbar; bestehende Funde bleiben unverändert",
        ));
    }
    let container_count = discovered.containers.len() as u32;
    if let Some(repositories) = state.repositories.as_ref() {
        for container in &discovered.containers {
            let workload = DockerWorkload {
                host_container_id: container_id,
                id: container.id.clone(),
                name: container.name.clone(),
                image: container.image.clone(),
                state: container.state.clone(),
                status: container.status.clone(),
                ports: container.ports.clone(),
                started_at: container.started_at.clone(),
                labels: container.labels.clone(),
                presence: lxcup_core::DockerWorkloadPresence::Present,
                change: lxcup_core::DockerWorkloadChange::New,
                management_state: DockerWorkloadManagementState::Discovered,
                discovered_at: chrono::Utc::now(),
            };
            if repositories
                .docker_workloads
                .upsert_discovered(&workload)
                .await
                .is_err()
            {
                finish_docker_discovery(&state, &mut run, 0, Some("docker_inventory_unavailable"))
                    .await?;
                return Err(ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht gespeichert werden",
                ));
            }
        }
        if repositories
            .docker_workloads
            .mark_missing_except(
                container_id,
                &discovered
                    .containers
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await
            .is_err()
        {
            finish_docker_discovery(&state, &mut run, 0, Some("docker_inventory_unavailable"))
                .await?;
            return Err(ApiError::dependency(
                "docker_inventory_unavailable",
                "Docker-Inventar konnte nicht abgeglichen werden",
            ));
        }
        let workloads = match repositories.docker_workloads.list(container_id).await {
            Ok(workloads) => workloads.into_iter().map(DockerWorkloadDto::from).collect(),
            Err(_) => {
                finish_docker_discovery(&state, &mut run, 0, Some("docker_inventory_unavailable"))
                    .await?;
                return Err(ApiError::dependency(
                    "docker_inventory_unavailable",
                    "Docker-Inventar konnte nicht gelesen werden",
                ));
            }
        };
        let run = finish_docker_discovery(&state, &mut run, container_count, None).await?;
        return Ok(Json(envelope(DockerDiscoveryResultDto { run, workloads })));
    }
    let mut store = state.store.write().await;
    let seen_ids = discovered
        .containers
        .iter()
        .map(|container| container.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for container in discovered.containers {
        let key = (container_id, container.id.clone());
        let previous = store.docker_workloads.get(&key).cloned();
        let management_state = previous
            .as_ref()
            .map(|item| item.management_state.clone())
            .unwrap_or_else(|| "discovered".to_owned());
        let change_state = match previous {
            None => "new",
            Some(ref previous)
                if previous.presence == "missing"
                    || previous.name != container.name
                    || previous.image != container.image
                    || previous.state != container.state
                    || previous.status != container.status
                    || previous.ports != container.ports
                    || previous.started_at != container.started_at
                    || previous.labels != container.labels =>
            {
                "changed"
            }
            Some(_) => "unchanged",
        };
        let mut workload = DockerWorkloadDto::discovered(container_id, container);
        workload.management_state = management_state;
        workload.change_state = change_state.to_owned();
        store.docker_workloads.insert(key, workload);
    }
    for ((host_id, id), workload) in store.docker_workloads.iter_mut() {
        if *host_id == container_id && !seen_ids.contains(id) && workload.presence == "present" {
            workload.presence = "missing".to_owned();
            workload.change_state = "missing".to_owned();
        }
    }
    let workloads = store
        .docker_workloads
        .values()
        .filter(|item| item.host_container_id == container_id)
        .cloned()
        .collect();
    drop(store);
    let run = finish_docker_discovery(&state, &mut run, container_count, None).await?;
    Ok(Json(envelope(DockerDiscoveryResultDto { run, workloads })))
}

async fn finish_docker_discovery(
    state: &ApiState,
    run: &mut lxcup_core::DockerDiscoveryRun,
    container_count: u32,
    error_code: Option<&str>,
) -> Result<lxcup_core::DockerDiscoveryRun, ApiError> {
    let status = if error_code.is_some() {
        lxcup_core::DockerDiscoveryStatus::Failed
    } else {
        lxcup_core::DockerDiscoveryStatus::Succeeded
    };
    if let Some(repositories) = state.repositories.as_ref() {
        *run = repositories
            .docker_discovery
            .finish(run.id, status, container_count, error_code)
            .await
            .map_err(|_| ApiError::storage())?;
    } else {
        run.status = status;
        run.finished_at = Some(chrono::Utc::now());
        run.container_count = container_count;
        run.error_code = error_code.map(str::to_owned);
        if let Some(stored) = state
            .store
            .write()
            .await
            .docker_discovery_runs
            .iter_mut()
            .find(|stored| stored.id == run.id)
        {
            *stored = run.clone();
        }
    }
    state.publish(ApiEvent::status(
        "docker_discovery",
        run.host_container_id.value().to_string(),
        if error_code.is_some() {
            "failed"
        } else {
            "succeeded"
        },
    ));
    if let Some(code) = error_code {
        state.publish(ApiEvent::Error {
            code: code.to_owned(),
            message: "Docker discovery failed".to_owned(),
            request_id: run.id.to_string(),
        });
    }
    Ok(run.clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_store_errors_map_to_stable_api_categories() {
        let cases = [
            (
                SecretStoreError::Missing,
                StatusCode::BAD_GATEWAY,
                "agent_secret_missing",
            ),
            (
                SecretStoreError::Denied,
                StatusCode::FORBIDDEN,
                "agent_secret_denied",
            ),
            (
                SecretStoreError::Invalid,
                StatusCode::BAD_REQUEST,
                "agent_secret_invalid",
            ),
            (
                SecretStoreError::Unavailable,
                StatusCode::BAD_GATEWAY,
                "secret_store_unavailable",
            ),
        ];
        for (error, status, code) in cases {
            let mapped = map_secret_error(error);
            assert_eq!(mapped.status, status);
            assert_eq!(mapped.code, code);
            assert!(!mapped.message.is_empty());
        }
    }

    #[test]
    fn secret_dtos_never_contain_secret_material() {
        let error = map_secret_error(SecretStoreError::Invalid);
        let rendered = format!("{error:?}");
        assert!(!rendered.contains("secret value"));
    }

    #[test]
    fn docker_workload_dto_preserves_discovered_and_managed_states() {
        let discovered = DockerWorkloadDto::discovered(
            ContainerId::new(101),
            DockerContainerInfo {
                id: "docker-id".to_owned(),
                name: "web".to_owned(),
                image: "nginx:latest".to_owned(),
                state: "running".to_owned(),
                status: "Up".to_owned(),
                ports: vec!["80/tcp".to_owned()],
                started_at: Some("2026-01-01T00:00:00Z".to_owned()),
                labels: vec!["app=web".to_owned()],
            },
        );
        assert_eq!(discovered.management_state, "discovered");
        let managed = DockerWorkload {
            host_container_id: ContainerId::new(101),
            id: discovered.id.clone(),
            name: discovered.name.clone(),
            image: discovered.image.clone(),
            state: discovered.state.clone(),
            status: discovered.status.clone(),
            ports: discovered.ports.clone(),
            started_at: discovered.started_at.clone(),
            labels: discovered.labels.clone(),
            presence: lxcup_core::DockerWorkloadPresence::Present,
            change: lxcup_core::DockerWorkloadChange::Changed,
            management_state: DockerWorkloadManagementState::Managed,
            discovered_at: discovered.discovered_at,
        };
        assert_eq!(DockerWorkloadDto::from(managed).management_state, "managed");
    }
}
