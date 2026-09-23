use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, CreateEnrollmentRequest, Enrollment, EnrollmentDto,
    envelope, parse_uuid,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters,
    ExecutionMode, JobEvent, JobEventKind, JobFailureCode, JobSubmission,
};
use lxcup_core::{
    ActorRole, ContainerId, ContainerManagementState, EnrollmentId, EnrollmentState,
    ResourceLifecycle, ResourceTarget, SecretId, TargetId, TargetState,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(super) async fn create_enrollment(
    State(state): State<ApiState>,
    JsonBody(request): JsonBody<CreateEnrollmentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<EnrollmentDto>>), ApiError> {
    let container_id = ContainerId::new(request.container_id);
    if request.container_id == 0 {
        return Err(ApiError::bad_request(
            "invalid_container_id",
            "container id must be greater than zero",
        ));
    }

    let mut store = state.store.write().await;
    if !store
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }

    if let Some(existing_id) = store.enrollment_keys.get(&request.idempotency_key).copied() {
        let existing = store
            .enrollments
            .iter()
            .find(|enrollment| enrollment.id == existing_id)
            .expect("enrollment idempotency index must point to an enrollment");
        if existing.container_id != container_id {
            return Err(ApiError::conflict(
                "idempotency_key_reused",
                "idempotency key is already used for another container",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(envelope(EnrollmentDto::from(existing))),
        ));
    }

    if store
        .enrollments
        .iter()
        .any(|enrollment| enrollment.container_id == container_id && enrollment.state.is_active())
    {
        return Err(ApiError::conflict(
            "enrollment_in_progress",
            "an enrollment is already in progress for this container",
        ));
    }

    let enrollment =
        Enrollment::new(container_id, request.idempotency_key.clone()).map_err(|_| {
            ApiError::bad_request("invalid_idempotency_key", "idempotency key is invalid")
        })?;
    let dto = EnrollmentDto::from(&enrollment);
    store
        .enrollment_keys
        .insert(request.idempotency_key.clone(), enrollment.id);
    store.enrollments.push(enrollment);
    drop(store);

    if request.start_onboarding {
        queue_enrollment_job(&state, &request, &dto).await?;
    }

    state.publish(ApiEvent::status(
        "enrollment",
        dto.id.as_uuid().to_string(),
        "requested",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

pub(super) async fn get_enrollment(
    State(state): State<ApiState>,
    Path(enrollment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnrollmentDto>>, ApiError> {
    let enrollment_id = EnrollmentId::from_uuid(parse_uuid(&enrollment_id, "enrollment id")?);
    let enrollment_exists = state
        .store
        .read()
        .await
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .is_some();
    if !enrollment_exists {
        return Err(ApiError::not_found("enrollment not found"));
    }

    // Reconcile the enrollment view with the deployment job. This keeps the
    // onboarding page from waiting forever when the worker has already
    // finished (or rejected) the linked agent deployment.
    let job_key = format!("enrollment-{}", enrollment_id.as_uuid());
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    if let Some(job) = jobs.iter().find(|job| job.idempotency_key == job_key) {
        let target_connected = if let ResourceTarget::Target(target_id) = job.target {
            state
                .store
                .read()
                .await
                .targets
                .iter()
                .any(|target| target.id == target_id && target.state == TargetState::Managed)
        } else {
            false
        };
        let mut store = state.store.write().await;
        if let Some(current) = store
            .enrollments
            .iter_mut()
            .find(|item| item.id == enrollment_id)
        {
            match job.status {
                AnsibleJobStatus::Failed => current
                    .fail("Agent-Deployment fehlgeschlagen. Details stehen im Workflow-Protokoll."),
                AnsibleJobStatus::Applying | AnsibleJobStatus::Succeeded
                    if current.state == EnrollmentState::Discovering =>
                {
                    let _ = current.transition_to(EnrollmentState::InstallingAgent);
                }
                AnsibleJobStatus::Succeeded if target_connected => {
                    let _ = current.transition_to(EnrollmentState::RegisteringAgent);
                    let _ = current.transition_to(EnrollmentState::Connected);
                }
                _ => {}
            }
        }
    }
    let store = state.store.read().await;
    let enrollment = store
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .ok_or_else(|| ApiError::not_found("enrollment not found"))?;
    Ok(Json(envelope(EnrollmentDto::from(enrollment))))
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateAnsibleJobRequest {
    pub operation: AnsibleOperation,
    #[serde(default)]
    pub target_id: Option<TargetId>,
    #[serde(default)]
    pub container_id: u64,
    pub mode: ExecutionMode,
    pub parameters: AnsibleParameters,
    pub idempotency_key: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnsibleJobDto {
    pub id: lxcup_core::AnsibleJobId,
    pub operation: AnsibleOperation,
    pub playbook: String,
    pub playbook_version: String,
    pub target: ResourceTarget,
    pub mode: ExecutionMode,
    pub status: AnsibleJobStatus,
    pub parameter_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&AnsibleJob> for AnsibleJobDto {
    fn from(job: &AnsibleJob) -> Self {
        Self {
            id: job.id,
            operation: job.operation,
            playbook: job.playbook.clone(),
            playbook_version: job.playbook_version.clone(),
            target: job.target,
            mode: job.mode,
            status: job.status,
            parameter_hash: job.parameter_hash.clone(),
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

pub(super) async fn list_ansible_jobs(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<AnsibleJobDto>>>, ApiError> {
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    Ok(Json(envelope(
        jobs.iter().map(AnsibleJobDto::from).collect(),
    )))
}

async fn queue_enrollment_job(
    state: &ApiState,
    request: &CreateEnrollmentRequest,
    dto: &EnrollmentDto,
) -> Result<(), ApiError> {
    let Some(target_id) = request.target_id else {
        return Ok(());
    };
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    let lifecycle = match target.state {
        TargetState::Pending => ResourceLifecycle::Pending,
        TargetState::Managed => ResourceLifecycle::Managed,
        TargetState::Disabled => ResourceLifecycle::Disabled,
    };
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: "0.1.0".to_owned(),
            },
            secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
            idempotency_key: format!("enrollment-{}", dto.id.as_uuid()),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .map_err(|_| {
            ApiError::conflict(
                "enrollment_job_rejected",
                "agent deployment could not be queued",
            )
        })?;
    let (job, is_created) = match submission {
        JobSubmission::Created(job) => (job, true),
        JobSubmission::Duplicate(job) => (job, false),
    };
    if !is_created {
        return Ok(());
    }
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .save(&job)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    let mut store = state.store.write().await;
    if let Some(enrollment) = store.enrollments.iter_mut().find(|item| item.id == dto.id) {
        enrollment
            .transition_to(EnrollmentState::Discovering)
            .map_err(|_| {
                ApiError::conflict(
                    "enrollment_invalid_state",
                    "enrollment cannot start discovery",
                )
            })?;
    }
    state.publish(ApiEvent::status(
        "ansible_job",
        job.id.as_uuid().to_string(),
        "queued",
    ));
    Ok(())
}

pub(super) async fn create_ansible_job(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateAnsibleJobRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<AnsibleJobDto>>), ApiError> {
    let (target, lifecycle, target_secret_ref) = resolve_ansible_target(&state, &request).await?;
    let secret_refs = resolve_job_secret_refs(request.operation, target_secret_ref)?;
    let job_request = AnsibleJobRequest {
        operation: request.operation,
        target,
        lifecycle,
        mode: request.mode,
        parameters: request.parameters,
        secret_refs,
        idempotency_key: request.idempotency_key,
        confirmed: request.confirmed,
        actor_role,
    };
    let target = job_request.target;
    if let Some(existing) = find_existing_job(&state, target, &job_request.idempotency_key).await? {
        return Ok((
            StatusCode::OK,
            Json(envelope(AnsibleJobDto::from(&existing))),
        ));
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(job_request)
        .map_err(map_ansible_error)?;
    let (status, job, created) = match submission {
        JobSubmission::Created(job) => (StatusCode::ACCEPTED, job, true),
        JobSubmission::Duplicate(job) => (StatusCode::OK, job, false),
    };
    if created {
        persist_created_job(&state, &job).await?;
    }
    let dto = AnsibleJobDto::from(&job);
    state.publish(ApiEvent::status(
        "ansible_job",
        dto.id.as_uuid().to_string(),
        "queued",
    ));
    Ok((status, Json(envelope(dto))))
}

async fn resolve_ansible_target(
    state: &ApiState,
    request: &CreateAnsibleJobRequest,
) -> Result<(ResourceTarget, ResourceLifecycle, Option<SecretId>), ApiError> {
    if let Some(target_id) = request.target_id {
        let target = state
            .store
            .read()
            .await
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?;
        let lifecycle = match target.state {
            TargetState::Pending => ResourceLifecycle::Pending,
            TargetState::Managed => ResourceLifecycle::Managed,
            TargetState::Disabled => ResourceLifecycle::Disabled,
        };
        return Ok((
            ResourceTarget::Target(target_id),
            lifecycle,
            Some(target.credential_secret_ref),
        ));
    }
    if request.container_id == 0 {
        return Err(ApiError::bad_request(
            "target_required",
            "target id must be provided",
        ));
    }
    let container_id = ContainerId::new(request.container_id);
    let lifecycle = state
        .store
        .read()
        .await
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .map(|container| match container.management_state {
            ContainerManagementState::Discovered => ResourceLifecycle::Discovered,
            ContainerManagementState::Managed => ResourceLifecycle::Managed,
            ContainerManagementState::Ignored | ContainerManagementState::Disabled => {
                ResourceLifecycle::Disabled
            }
        })
        .ok_or_else(|| ApiError::not_found("container not found"))?;
    Ok((ResourceTarget::Container(container_id), lifecycle, None))
}

fn resolve_job_secret_refs(
    operation: AnsibleOperation,
    target_secret_ref: Option<SecretId>,
) -> Result<Vec<SecretId>, ApiError> {
    if operation == AnsibleOperation::HealthCheck {
        return Ok(Vec::new());
    }
    target_secret_ref.map_or_else(configured_ansible_secret_refs, |secret| Ok(vec![secret]))
}

async fn find_existing_job(
    state: &ApiState,
    target: ResourceTarget,
    idempotency_key: &str,
) -> Result<Option<AnsibleJob>, ApiError> {
    let Some(repositories) = state.repositories.clone() else {
        return Ok(None);
    };
    if let Some(existing) = repositories
        .ansible_jobs
        .find_by_idempotency_key(target, idempotency_key)
        .await
        .map_err(|_| ApiError::storage())?
    {
        return Ok(Some(existing));
    }
    if repositories
        .ansible_jobs
        .has_active_target(target)
        .await
        .map_err(|_| ApiError::storage())?
    {
        return Err(ApiError::conflict(
            "ansible_target_busy",
            "another job is active for this target",
        ));
    }
    Ok(None)
}

async fn persist_created_job(state: &ApiState, job: &AnsibleJob) -> Result<(), ApiError> {
    let Some(repositories) = state.repositories.clone() else {
        return Ok(());
    };
    repositories
        .ansible_jobs
        .save(job)
        .await
        .map_err(|_| ApiError::storage())?;
    let events = state
        .ansible
        .read()
        .await
        .events(job.id)
        .map_err(map_ansible_error)?;
    for event in events {
        repositories
            .ansible_jobs
            .append_event(&event)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(())
}

pub(super) async fn get_ansible_job(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiEnvelope<AnsibleJobDto>>, ApiError> {
    let id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    let job = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .find_by_id(id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("ansible job not found"))?
    } else {
        state
            .ansible
            .read()
            .await
            .job(id)
            .map_err(map_ansible_error)?
    };
    Ok(Json(envelope(AnsibleJobDto::from(&job))))
}

pub(super) async fn get_ansible_job_events(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<lxcup_ansible::JobEvent>>>, ApiError> {
    let id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    if let Some(repositories) = state.repositories.clone() {
        let mut events = repositories
            .ansible_jobs
            .events(id)
            .await
            .map_err(|_| ApiError::storage())?;
        let job = repositories
            .ansible_jobs
            .find_by_id(id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("ansible job not found"))?;
        if job.status == AnsibleJobStatus::Failed
            && !events
                .iter()
                .any(|event| matches!(&event.event, JobEventKind::Failed { .. }))
        {
            let event = JobEvent {
                sequence: events.len() as u64 + 1,
                job_id: id,
                event: JobEventKind::Failed {
                    code: JobFailureCode::WorkerUnavailable,
                },
                created_at: chrono::Utc::now(),
            };
            repositories
                .ansible_jobs
                .append_event(&event)
                .await
                .map_err(|_| ApiError::storage())?;
            events.push(event);
        }
        Ok(Json(envelope(events)))
    } else {
        state
            .ansible
            .read()
            .await
            .events(id)
            .map(|events| Json(envelope(events)))
            .map_err(map_ansible_error)
    }
}

pub(super) fn configured_ansible_secret_refs() -> Result<Vec<SecretId>, ApiError> {
    let raw = std::env::var("LXCUP_ANSIBLE_SECRET_IDS").map_err(|_| {
        ApiError::dependency(
            "secret_reference_missing",
            "no Ansible credential is configured",
        )
    })?;
    let refs = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Uuid::parse_str(value)
                .map(SecretId::from_uuid)
                .map_err(|_| {
                    ApiError::bad_request("invalid_secret_reference", "secret reference is invalid")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if refs.is_empty() {
        return Err(ApiError::dependency(
            "secret_reference_missing",
            "no Ansible credential is configured",
        ));
    }
    Ok(refs)
}

pub(super) fn map_ansible_error(error: lxcup_ansible::CoordinatorError) -> ApiError {
    match error {
        lxcup_ansible::CoordinatorError::TargetBusy => ApiError::conflict(
            "ansible_target_busy",
            "another job is active for this target",
        ),
        lxcup_ansible::CoordinatorError::NotFound => ApiError::not_found("ansible job not found"),
        lxcup_ansible::CoordinatorError::RetryNotAllowed
        | lxcup_ansible::CoordinatorError::InvalidTransition
        | lxcup_ansible::CoordinatorError::Terminal => {
            ApiError::bad_request("ansible_job_invalid", "the Ansible job state is invalid")
        }
        lxcup_ansible::CoordinatorError::Contract(error) => match error {
            lxcup_ansible::AnsibleContractError::PermissionDenied => ApiError::forbidden(
                "ansible_permission_denied",
                "the role cannot run this operation",
            ),
            lxcup_ansible::AnsibleContractError::ConfirmationRequired => ApiError::bad_request(
                "ansible_confirmation_required",
                "the operation requires explicit confirmation",
            ),
            lxcup_ansible::AnsibleContractError::MissingSecretReference => ApiError::dependency(
                "secret_reference_missing",
                "the operation has no configured credential reference",
            ),
            lxcup_ansible::AnsibleContractError::UnsupportedTarget => ApiError::bad_request(
                "ansible_target_invalid",
                "the operation is not supported for this target",
            ),
            lxcup_ansible::AnsibleContractError::Invalid
            | lxcup_ansible::AnsibleContractError::InvalidTransition => {
                ApiError::bad_request("ansible_request_invalid", "the Ansible request is invalid")
            }
        },
    }
}
