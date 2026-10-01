use super::{
    ActorRole, AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation,
    AnsibleParameters, ApiEnvelope, ApiError, ApiEvent, ApiState, CreateEnrollmentRequest,
    EnrollmentDto, EnrollmentState, ExecutionMode, Extension, JobEventKind, JobFailureCode,
    JobSubmission, Json, JsonBody, Path, PlaybookRegistry, ResourceLifecycle, ResourceTarget,
    SecretId, State, StatusCode, TargetId, TargetState, Uuid, envelope, find_existing_job,
    find_idempotent_job, parse_uuid, require_permission, resolve_ansible_target,
    resolve_job_secret_refs, validate_package_update,
};
use lxcup_ansible::JobEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateAnsibleJobRequest {
    pub operation: AnsibleOperation,
    #[serde(default)]
    pub target_id: Option<TargetId>,
    #[serde(default)]
    pub container_id: u64,
    pub mode: ExecutionMode,
    pub parameters: AnsibleParameters,
    pub idempotency_key: String,
    pub confirmed: bool,
    #[serde(default)]
    pub policy_id: Option<String>,
    #[serde(default)]
    pub approved_plan_job_id: Option<lxcup_core::AnsibleJobId>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_plan_job_id: Option<lxcup_core::AnsibleJobId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciles_job_id: Option<lxcup_core::AnsibleJobId>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&AnsibleJob> for AnsibleJobDto {
    fn from(job: &AnsibleJob) -> Self {
        let package_names = match &job.parameters {
            AnsibleParameters::UpdatePackages { packages } => Some(packages.clone()),
            _ => None,
        };
        let update_policy_id = job
            .idempotency_key
            .strip_prefix("package-plan:")
            .and_then(|value| value.rsplit_once(':'))
            .map(|(policy_id, _)| policy_id.to_owned());
        let approved_plan_job_id = job
            .idempotency_key
            .strip_prefix("package-apply:")
            .and_then(|value| value.split_once(':'))
            .and_then(|(id, _)| Uuid::parse_str(id).ok())
            .map(lxcup_core::AnsibleJobId::from_uuid);
        Self {
            id: job.id,
            operation: job.operation,
            playbook: job.playbook.clone(),
            playbook_version: job.playbook_version.clone(),
            target: job.target,
            mode: job.mode,
            status: job.status,
            parameter_hash: job.parameter_hash.clone(),
            package_names,
            update_policy_id,
            approved_plan_job_id,
            reconciles_job_id: lxcup_ansible::reconciliation_source_job_id(&job.idempotency_key),
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

pub(crate) async fn list_ansible_jobs(
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

pub(super) async fn queue_enrollment_job(
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
                agent_version: "0.3.1".to_owned(),
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

pub(crate) async fn create_ansible_job(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateAnsibleJobRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<AnsibleJobDto>>), ApiError> {
    let (target, lifecycle, target_secret_ref) = resolve_ansible_target(&state, &request).await?;
    validate_package_update(&state, &request, target).await?;
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
    if let Some(repositories) = state.repositories.clone() {
        if repositories
            .ansible_jobs
            .has_reconciliation_required_target(target)
            .await
            .map_err(|_| ApiError::storage())?
        {
            let blocking = find_active_target_job(&state, target, None).await?;
            if let Some(blocking) = blocking {
                return Err(ApiError::conflict_with_blocking_job(
                    "ansible_target_busy",
                    "target requires reconciliation before another workflow can start",
                    blocking.id.as_uuid(),
                    job_status_name(blocking.status),
                ));
            }
            return Err(ApiError::conflict(
                "ansible_target_busy",
                "target requires reconciliation before another workflow can start",
            ));
        }
    }
    if let Some(blocking) = find_active_target_job(&state, target, None).await? {
        return Err(ApiError::conflict_with_blocking_job(
            "ansible_target_busy",
            "another job is active for this target",
            blocking.id.as_uuid(),
            job_status_name(blocking.status),
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

pub(crate) async fn persist_created_job(
    state: &ApiState,
    job: &AnsibleJob,
) -> Result<(), ApiError> {
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

pub(crate) async fn get_ansible_job(
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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RetryAnsibleJobRequest {
    #[serde(default)]
    confirmed: bool,
}

pub(crate) async fn retry_ansible_job(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(job_id): Path<String>,
    JsonBody(request): JsonBody<RetryAnsibleJobRequest>,
) -> Result<Json<ApiEnvelope<AnsibleJobDto>>, ApiError> {
    let id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    let existing = if let Some(repositories) = state.repositories.clone() {
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
    let spec = PlaybookRegistry.resolve(existing.operation);
    require_permission(actor_role, spec.action.permission())?;
    if existing.mode == ExecutionMode::Apply
        && spec.action.permission() != lxcup_core::Permission::Read
        && !request.confirmed
    {
        return Err(ApiError::bad_request(
            "ansible_confirmation_required",
            "retrying a mutating operation requires explicit confirmation",
        ));
    }

    let retried = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .retry(id)
            .await
            .map_err(|error| match error {
                lxcup_persistence::RepositoryError::InvalidValue { .. } => ApiError::bad_request(
                    "ansible_retry_not_allowed",
                    "this job is not in a retryable state",
                ),
                lxcup_persistence::RepositoryError::Database(_) => ApiError::conflict(
                    "ansible_target_busy",
                    "another job is executing for this target",
                ),
                lxcup_persistence::RepositoryError::Serialization(_) => ApiError::storage(),
                lxcup_persistence::RepositoryError::TargetBusy => ApiError::conflict(
                    "ansible_target_busy",
                    "another job is executing for this target",
                ),
                lxcup_persistence::RepositoryError::LastAdmin => ApiError::storage(),
                lxcup_persistence::RepositoryError::UsernameExists => ApiError::storage(),
            })?
    } else {
        state
            .ansible
            .write()
            .await
            .retry(id)
            .map_err(map_ansible_error)?
    };
    state.publish(ApiEvent::status(
        "ansible_job",
        id.as_uuid().to_string(),
        "queued",
    ));
    Ok(Json(envelope(AnsibleJobDto::from(&retried))))
}

pub(crate) async fn reconcile_ansible_job(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(job_id): Path<String>,
) -> Result<(StatusCode, Json<ApiEnvelope<AnsibleJobDto>>), ApiError> {
    let source_id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "ansible job id")?);
    let source = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .find_by_id(source_id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("ansible job not found"))?
    } else {
        state
            .ansible
            .read()
            .await
            .job(source_id)
            .map_err(map_ansible_error)?
    };
    require_permission(
        actor_role,
        PlaybookRegistry
            .resolve(source.operation)
            .action
            .permission(),
    )?;
    if !source.can_be_reconciled() {
        return Err(map_ansible_error(
            lxcup_ansible::CoordinatorError::ReconcileNotAllowed,
        ));
    }
    let idempotency_key = lxcup_ansible::reconcile_idempotency_key(source_id);
    if let Some(existing) = find_idempotent_job(&state, source.target, &idempotency_key).await? {
        return Ok((
            StatusCode::OK,
            Json(envelope(AnsibleJobDto::from(&existing))),
        ));
    }
    let persistent = state.repositories.clone();
    if let Some(blocking) = find_active_target_job(&state, source.target, Some(source.id)).await? {
        return Err(ApiError::conflict_with_blocking_job(
            "ansible_target_busy",
            "another job is executing for this target",
            blocking.id.as_uuid(),
            job_status_name(blocking.status),
        ));
    }
    let submission = if persistent.is_some() {
        state
            .ansible
            .write()
            .await
            .submit_persisted_reconciliation(&source)
    } else {
        state.ansible.write().await.submit_reconciliation(&source)
    }
    .map_err(map_ansible_error)?;
    let (status, job, created) = match submission {
        JobSubmission::Created(job) => (StatusCode::ACCEPTED, job, true),
        JobSubmission::Duplicate(job) if persistent.is_some() => (StatusCode::ACCEPTED, job, true),
        JobSubmission::Duplicate(job) => (StatusCode::OK, job, false),
    };
    if created {
        persist_created_job(&state, &job).await?;
    }
    state.publish(ApiEvent::status(
        "ansible_job",
        job.id.as_uuid().to_string(),
        "queued",
    ));
    Ok((status, Json(envelope(AnsibleJobDto::from(&job)))))
}

async fn find_active_target_job(
    state: &ApiState,
    target: ResourceTarget,
    excluded_job_id: Option<lxcup_core::AnsibleJobId>,
) -> Result<Option<AnsibleJob>, ApiError> {
    let jobs = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    Ok(jobs
        .into_iter()
        .filter(|job| {
            job.target == target
                && Some(job.id) != excluded_job_id
                && is_active_job_status(job.status)
        })
        .max_by_key(|job| job.updated_at))
}

fn is_active_job_status(status: AnsibleJobStatus) -> bool {
    matches!(
        status,
        AnsibleJobStatus::Checking
            | AnsibleJobStatus::Planned
            | AnsibleJobStatus::Applying
            | AnsibleJobStatus::ReconcileRequired
    )
}

const fn job_status_name(status: AnsibleJobStatus) -> &'static str {
    match status {
        AnsibleJobStatus::Queued => "queued",
        AnsibleJobStatus::Checking => "checking",
        AnsibleJobStatus::Planned => "planned",
        AnsibleJobStatus::Applying => "applying",
        AnsibleJobStatus::ReconcileRequired => "reconcile_required",
        AnsibleJobStatus::Succeeded => "succeeded",
        AnsibleJobStatus::Failed => "failed",
        AnsibleJobStatus::Aborted => "aborted",
    }
}

pub(crate) async fn get_ansible_job_events(
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
        lxcup_ansible::CoordinatorError::ReconcileNotAllowed => ApiError::bad_request(
            "ansible_reconcile_not_allowed",
            "only an unresolved Apply job can be reconciled",
        ),
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
            lxcup_ansible::AnsibleContractError::UnsupportedMode => ApiError::bad_request(
                "ansible_mode_invalid",
                "the execution mode is not supported for this operation",
            ),
            lxcup_ansible::AnsibleContractError::Invalid
            | lxcup_ansible::AnsibleContractError::InvalidTransition => {
                ApiError::bad_request("ansible_request_invalid", "the Ansible request is invalid")
            }
        },
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
