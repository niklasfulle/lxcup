use super::*;

pub(super) async fn list_nodes(State(state): State<ApiState>) -> Json<ApiEnvelope<Vec<NodeDto>>> {
    let nodes = state
        .store
        .read()
        .await
        .nodes
        .iter()
        .map(NodeDto::from)
        .collect();
    Json(envelope(nodes))
}

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
    let targets = state
        .store
        .read()
        .await
        .targets
        .iter()
        .map(TargetDto::from)
        .collect();
    Json(envelope(targets))
}

pub(super) async fn create_target(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateTargetRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<TargetDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let mut target = Target::new(
        request.name,
        request.kind,
        request.address,
        request.transport,
        request.credential_secret_ref,
        request.agent_secret_ref,
    )
    .map_err(|_| {
        ApiError::bad_request("invalid_target", "target fields or transport are invalid")
    })?;
    target.ssh_user = request.ssh_user.filter(|value| !value.trim().is_empty());
    target.ssh_known_hosts_secret_ref = request.ssh_known_hosts_secret_ref;
    let dto = TargetDto::from(&target);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .save(&target)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.store.write().await.targets.push(target);
    state.publish(ApiEvent::status(
        "target",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

pub(super) async fn get_target(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<TargetDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let store = state.store.read().await;
    let target = store
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    Ok(Json(envelope(TargetDto::from(target))))
}

pub(super) async fn receive_agent_heartbeat(
    State(state): State<ApiState>,
    headers: HeaderMap,
    JsonBody(heartbeat): JsonBody<AgentHeartbeat>,
) -> Result<StatusCode, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let mut store = state.store.write().await;
    let target = store
        .targets
        .iter_mut()
        .find(|target| target.id.as_uuid() == heartbeat.target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    let expected = state
        .secrets
        .read(target.agent_secret_ref)
        .map_err(map_secret_error)?;
    if expected.expose() != token {
        return Err(ApiError::unauthorized());
    }
    target.mark_managed();
    let target_to_persist = target.clone();
    let target_id = target.id;
    store.agent_reports.insert(target_id, heartbeat);
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .update(&target_to_persist)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(ApiEvent::status(
        "target",
        target_id.as_uuid().to_string(),
        "agent_connected",
    ));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    pub endpoint: String,
    pub api_secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateEnvironmentRequest {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub api_secret_ref: Option<SecretId>,
    pub ca_secret_ref: Option<SecretId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConfirmedRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentDto {
    pub id: EnvironmentId,
    pub name: String,
    pub endpoint: String,
    pub api_secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
    pub status: EnvironmentStatus,
    pub last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_check_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ProxmoxEnvironment> for EnvironmentDto {
    fn from(environment: &ProxmoxEnvironment) -> Self {
        Self {
            id: environment.id,
            name: environment.name.clone(),
            endpoint: environment.endpoint.clone(),
            api_secret_ref: environment.api_secret_ref,
            ca_secret_ref: environment.ca_secret_ref,
            status: environment.status,
            last_checked_at: environment.last_checked_at,
            last_check_error: environment.last_check_error.clone(),
            created_at: environment.created_at,
            updated_at: environment.updated_at,
        }
    }
}

pub(super) async fn list_environments(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<EnvironmentDto>>>, ApiError> {
    let environments = if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.environments.clone()
    };
    Ok(Json(envelope(
        environments.iter().map(EnvironmentDto::from).collect(),
    )))
}

pub(super) async fn create_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<EnvironmentDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let now = chrono::Utc::now();
    let environment = ProxmoxEnvironment::new(
        EnvironmentId::new(),
        request.name,
        request.endpoint,
        request.api_secret_ref,
        request.ca_secret_ref,
        now,
    )
    .map_err(map_environment_domain_error)?;
    let dto = EnvironmentDto::from(&environment);
    let store = state.store.read().await;
    if store
        .environments
        .iter()
        .any(|item| item.name.eq_ignore_ascii_case(&environment.name))
    {
        return Err(ApiError::conflict(
            "environment_name_exists",
            "an environment with this name already exists",
        ));
    }
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .save(&environment)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    let mut store = state.store.write().await;
    store.environments.push(environment);
    drop(store);
    state.publish(ApiEvent::status(
        "environment",
        dto.id.as_uuid().to_string(),
        "configured",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

pub(super) async fn get_environment(
    State(state): State<ApiState>,
    Path(environment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    let id = parse_environment_id(&environment_id)?;
    let store = state.store.read().await;
    let environment = store
        .environments
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("environment not found"))?;
    Ok(Json(envelope(EnvironmentDto::from(&*environment))))
}

pub(super) async fn update_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<UpdateEnvironmentRequest>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let updated = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        let name = request.name.unwrap_or_else(|| environment.name.clone());
        let endpoint = request
            .endpoint
            .unwrap_or_else(|| environment.endpoint.clone());
        let api_secret_ref = request.api_secret_ref.unwrap_or(environment.api_secret_ref);
        environment
            .update_configuration(
                name,
                endpoint,
                api_secret_ref,
                request.ca_secret_ref.or(environment.ca_secret_ref),
                now,
            )
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&updated)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&updated))))
}

pub(super) async fn disable_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<ConfirmedRequest>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "disabling an environment requires explicit confirmation",
        ));
    }
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let disabled = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        environment
            .transition_to(EnvironmentStatus::Disabled, now)
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&disabled)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&disabled))))
}

pub(super) async fn delete_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
    JsonBody(request): JsonBody<ConfirmedRequest>,
) -> Result<StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "deleting an environment requires explicit confirmation",
        ));
    }
    let id = parse_environment_id(&environment_id)?;
    let mut store = state.store.write().await;
    let position = store
        .environments
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("environment not found"))?;
    store.environments.remove(position);
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .delete(id)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn check_environment(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(environment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnvironmentDto>>, ApiError> {
    require_permission(actor_role, Permission::Read)?;
    let id = parse_environment_id(&environment_id)?;
    let now = chrono::Utc::now();
    let checked = {
        let mut store = state.store.write().await;
        let environment = store
            .environments
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| ApiError::not_found("environment not found"))?;
        // The adapter/secret-store call is intentionally a separate worker seam.
        // Until that seam is wired, record a diagnosable, non-successful check.
        environment
            .record_check_failure(now, "Proxmox capability check is not scheduled")
            .map_err(map_environment_domain_error)?;
        environment.clone()
    };
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .environments
            .update(&checked)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(Json(envelope(EnvironmentDto::from(&checked))))
}

pub(super) fn require_permission(role: ActorRole, permission: Permission) -> Result<(), ApiError> {
    if role.grants(permission) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "permission_denied",
            "the role cannot change this resource",
        ))
    }
}

pub(super) fn map_environment_domain_error(error: lxcup_core::DomainError) -> ApiError {
    match error {
        lxcup_core::DomainError::PermissionDenied => {
            ApiError::forbidden("permission_denied", "the role cannot change this resource")
        }
        lxcup_core::DomainError::EmptyValue { .. }
        | lxcup_core::DomainError::InvalidEnvironmentEndpoint
        | lxcup_core::DomainError::InvalidPackageName => ApiError::bad_request(
            "invalid_environment",
            "the environment configuration is invalid",
        ),
        lxcup_core::DomainError::ConfirmationRequired => {
            ApiError::bad_request("confirmation_required", "explicit confirmation is required")
        }
        lxcup_core::DomainError::InvalidStateTransition(_) => ApiError::conflict(
            "invalid_environment_state",
            "the environment state cannot change",
        ),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerActionStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Blocked,
}

#[derive(Clone, Debug)]
pub(crate) struct ContainerActionTask {
    id: Uuid,
    container_id: ContainerId,
    action: ContainerAction,
    status: ContainerActionStatus,
    idempotency_key: String,
    error: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateContainerActionRequest {
    pub action: ContainerAction,
    pub idempotency_key: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerActionTaskDto {
    pub id: Uuid,
    pub container_id: ContainerId,
    pub action: ContainerAction,
    pub status: ContainerActionStatus,
    pub idempotency_key: String,
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ContainerActionTask> for ContainerActionTaskDto {
    fn from(task: &ContainerActionTask) -> Self {
        Self {
            id: task.id,
            container_id: task.container_id,
            action: task.action,
            status: task.status,
            idempotency_key: task.idempotency_key.clone(),
            error: task.error.clone(),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

pub(super) async fn create_container_action(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(container_id): Path<String>,
    JsonBody(request): JsonBody<CreateContainerActionRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ContainerActionTaskDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if request.action.requires_confirmation() {
        require_permission(actor_role, Permission::Destructive)?;
    }
    let container_id = parse_container_id(&container_id)?;
    let mut store = state.store.write().await;
    let container = store
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("container not found"))?;
    container
        .validate_action(request.action, request.confirmed, &request.idempotency_key)
        .map_err(map_container_action_error)?;

    let key = (container_id, request.idempotency_key.clone());
    if let Some(existing_id) = store.container_action_keys.get(&key).copied() {
        let existing = store
            .container_actions
            .iter()
            .find(|task| task.id == existing_id)
            .expect("container action idempotency index must point to a task");
        if existing.action != request.action {
            return Err(ApiError::conflict(
                "idempotency_key_reused",
                "idempotency key is already used for another action",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(envelope(ContainerActionTaskDto::from(existing))),
        ));
    }

    if store.container_actions.iter().any(|task| {
        task.container_id == container_id && task.status == ContainerActionStatus::Running
    }) {
        return Err(ApiError::conflict(
            "container_action_busy",
            "another action is already running for this container",
        ));
    }

    let now = chrono::Utc::now();
    let task = ContainerActionTask {
        id: Uuid::new_v4(),
        container_id,
        action: request.action,
        status: ContainerActionStatus::Queued,
        idempotency_key: request.idempotency_key,
        error: None,
        created_at: now,
        updated_at: now,
    };
    let dto = ContainerActionTaskDto::from(&task);
    store
        .container_action_keys
        .insert((container_id, task.idempotency_key.clone()), task.id);
    store.container_actions.push(task);
    drop(store);
    state.publish(ApiEvent::status(
        "container_action",
        dto.id.to_string(),
        "queued",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

pub(super) async fn get_container_action(
    State(state): State<ApiState>,
    Path(action_id): Path<String>,
) -> Result<Json<ApiEnvelope<ContainerActionTaskDto>>, ApiError> {
    let action_id = parse_uuid(&action_id, "container action id")?;
    let store = state.store.read().await;
    let task = store
        .container_actions
        .iter()
        .find(|task| task.id == action_id)
        .ok_or_else(|| ApiError::not_found("container action not found"))?;
    Ok(Json(envelope(ContainerActionTaskDto::from(task))))
}

pub(super) fn map_container_action_error(error: lxcup_core::DomainError) -> ApiError {
    match error {
        lxcup_core::DomainError::ConfirmationRequired => ApiError::bad_request(
            "confirmation_required",
            "the container action requires explicit confirmation",
        ),
        lxcup_core::DomainError::EmptyValue { .. }
        | lxcup_core::DomainError::InvalidPackageName
        | lxcup_core::DomainError::InvalidEnvironmentEndpoint => ApiError::bad_request(
            "invalid_container_action",
            "the container action is invalid",
        ),
        lxcup_core::DomainError::InvalidStateTransition(_) => ApiError::conflict(
            "container_action_invalid_state",
            "the container status does not allow this action",
        ),
        lxcup_core::DomainError::PermissionDenied => {
            ApiError::forbidden("permission_denied", "the role cannot run this action")
        }
    }
}
