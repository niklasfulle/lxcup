use super::{
    ActorRole, AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation,
    AnsibleParameters, ApiEnvelope, ApiError, ApiEvent, ApiState, CreateEnrollmentRequest,
    EnrollmentDto, EnrollmentState, ExecutionMode, Extension, JobEventKind, JobFailureCode,
    JobSubmission, Json, JsonBody, Path, PlaybookRegistry, ResourceLifecycle, ResourceTarget,
    SecretId, State, StatusCode, TargetId, TargetState, Uuid, envelope, find_existing_job,
    parse_uuid, require_permission, resolve_ansible_target, resolve_job_secret_refs,
    validate_package_update,
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
                agent_version: "0.2.0".to_owned(),
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
mod tests {
    use super::*;
    use crate::{ContainerId, Enrollment};
    use lxcup_ansible::{AnsibleContractError, CoordinatorError};
    use std::sync::{Mutex, OnceLock};

    fn test_request(key: &str) -> AnsibleJobRequest {
        AnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target: ResourceTarget::Target(TargetId::new()),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            secret_refs: Vec::new(),
            idempotency_key: key.to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        }
    }

    #[test]
    fn coordinator_errors_keep_stable_api_codes() {
        let cases = [
            (CoordinatorError::TargetBusy, "ansible_target_busy"),
            (CoordinatorError::NotFound, "not_found"),
            (CoordinatorError::RetryNotAllowed, "ansible_job_invalid"),
            (CoordinatorError::InvalidTransition, "ansible_job_invalid"),
            (CoordinatorError::Terminal, "ansible_job_invalid"),
            (
                CoordinatorError::Contract(AnsibleContractError::PermissionDenied),
                "ansible_permission_denied",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::ConfirmationRequired),
                "ansible_confirmation_required",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::MissingSecretReference),
                "secret_reference_missing",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::UnsupportedTarget),
                "ansible_target_invalid",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::UnsupportedMode),
                "ansible_mode_invalid",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::Invalid),
                "ansible_request_invalid",
            ),
            (
                CoordinatorError::Contract(AnsibleContractError::InvalidTransition),
                "ansible_request_invalid",
            ),
        ];

        for (error, expected_code) in cases {
            assert_eq!(map_ansible_error(error).code, expected_code);
        }
    }

    #[tokio::test]
    async fn job_queries_and_retry_cover_in_memory_lifecycle() {
        let state = ApiState::new();
        let submission = state
            .ansible
            .write()
            .await
            .submit(test_request("jobs-handler-test"))
            .unwrap();
        let JobSubmission::Created(job) = submission else {
            panic!("first request should create a job")
        };

        let listed = list_ansible_jobs(State(state.clone())).await.unwrap();
        assert_eq!(listed.0.data.len(), 1);
        let queried = get_ansible_job(State(state.clone()), Path(job.id.as_uuid().to_string()))
            .await
            .unwrap();
        assert_eq!(queried.0.data.id, job.id);
        let events =
            get_ansible_job_events(State(state.clone()), Path(job.id.as_uuid().to_string()))
                .await
                .unwrap();
        assert!(!events.0.data.is_empty());

        state
            .ansible
            .write()
            .await
            .transition(job.id, AnsibleJobStatus::Checking)
            .unwrap();
        state
            .ansible
            .write()
            .await
            .transition(job.id, AnsibleJobStatus::Failed)
            .unwrap();
        let retried = retry_ansible_job(
            State(state.clone()),
            Extension(ActorRole::Viewer),
            Path(job.id.as_uuid().to_string()),
            JsonBody(RetryAnsibleJobRequest { confirmed: false }),
        )
        .await
        .unwrap();
        assert_eq!(retried.0.data.status, AnsibleJobStatus::Queued);
        assert!(
            get_ansible_job(State(state), Path("not-a-uuid".to_owned()),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_target_and_unconfigured_secret_are_reported_without_mutation() {
        let state = ApiState::new();
        let request = CreateAnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target_id: Some(TargetId::new()),
            container_id: 0,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            idempotency_key: "missing-target".to_owned(),
            confirmed: true,
            policy_id: None,
            approved_plan_job_id: None,
        };
        let error = create_ansible_job(
            State(state.clone()),
            Extension(ActorRole::Operator),
            JsonBody(request),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert!(state.ansible.read().await.jobs().is_empty());

        let _lock = env_lock().lock().unwrap();
        let old_value = std::env::var("LXCUP_ANSIBLE_SECRET_IDS").ok();
        unsafe_env_remove();
        assert_eq!(
            configured_ansible_secret_refs().unwrap_err().code,
            "secret_reference_missing"
        );
        unsafe { std::env::set_var("LXCUP_ANSIBLE_SECRET_IDS", "not-a-uuid") };
        assert_eq!(
            configured_ansible_secret_refs().unwrap_err().code,
            "invalid_secret_reference"
        );
        let first = SecretId::new();
        let second = SecretId::new();
        unsafe {
            std::env::set_var(
                "LXCUP_ANSIBLE_SECRET_IDS",
                format!(" {},, {} ", first.as_uuid(), second.as_uuid()),
            );
        }
        assert_eq!(
            configured_ansible_secret_refs().unwrap(),
            vec![first, second]
        );
        match old_value {
            Some(value) => unsafe { std::env::set_var("LXCUP_ANSIBLE_SECRET_IDS", value) },
            None => unsafe_env_remove(),
        }
    }

    #[tokio::test]
    async fn no_repository_persistence_and_optional_enrollment_target_are_noops() {
        let state = ApiState::new();
        let job = state
            .ansible
            .write()
            .await
            .submit(test_request("no-repository-persist"))
            .unwrap();
        let JobSubmission::Created(job) = job else {
            panic!("request should create a job")
        };
        persist_created_job(&state, &job).await.unwrap();
        let enrollment_request = CreateEnrollmentRequest {
            container_id: 1,
            target_id: None,
            idempotency_key: "without-target".to_owned(),
            start_onboarding: true,
        };
        let enrollment = Enrollment::new(ContainerId::new(1), "without-target".to_owned()).unwrap();
        let dto = EnrollmentDto::from(&enrollment);
        queue_enrollment_job(&state, &enrollment_request, &dto)
            .await
            .unwrap();
        assert_eq!(state.ansible.read().await.jobs().len(), 1);
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unsafe_env_remove() {
        unsafe { std::env::remove_var("LXCUP_ANSIBLE_SECRET_IDS") };
    }
}
