use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, CreateEnrollmentRequest, Enrollment, EnrollmentDto,
    envelope, parse_uuid, require_permission,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters,
    ExecutionMode, JobEvent, JobEventKind, JobFailureCode, JobSubmission, PlaybookRegistry,
};
use lxcup_core::{
    ActorRole, ContainerId, ContainerManagementState, EnrollmentId, EnrollmentState,
    ResourceLifecycle, ResourceTarget, SecretId, Target, TargetId, TargetState, UpdateRisk,
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
        drop(store);
        ensure_enrollment_followups(&state, enrollment_id, job).await?;
    }
    let store = state.store.read().await;
    let enrollment = store
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .ok_or_else(|| ApiError::not_found("enrollment not found"))?;
    Ok(Json(envelope(EnrollmentDto::from(enrollment))))
}

/// The controller owns the onboarding sequence so a browser reload or a
/// stopped UI cannot prevent the healthcheck and first inventory from being
/// queued. Idempotency keys make this safe to call on every enrollment poll.
async fn ensure_enrollment_followups(
    state: &ApiState,
    enrollment_id: EnrollmentId,
    deployment: &AnsibleJob,
) -> Result<(), ApiError> {
    ensure_deployment_followups(
        state,
        deployment,
        format!("enrollment-health-{}", enrollment_id.as_uuid()),
        format!("enrollment-packages-{}", enrollment_id.as_uuid()),
    )
    .await
    .map(|_| ())
}

/// Reconciles persisted successful deployments so onboarding can continue
/// after a browser reload or controller restart. The idempotency keys are
/// stable per target/enrollment, so polling and competing controller ticks
/// cannot create duplicate follow-up jobs.
pub(super) async fn reconcile_onboarding_jobs(state: &ApiState) -> Result<usize, ApiError> {
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };

    let mut queued = 0;
    for deployment in jobs.iter().filter(|job| {
        job.operation == AnsibleOperation::DeployAgent && job.status == AnsibleJobStatus::Succeeded
    }) {
        let (health_key, inventory_key) =
            if let Some(enrollment_id) = deployment.idempotency_key.strip_prefix("enrollment-") {
                (
                    format!("enrollment-health-{enrollment_id}"),
                    format!("enrollment-packages-{enrollment_id}"),
                )
            } else if let Some(target_id) = deployment
                .idempotency_key
                .strip_prefix("onboarding-deploy-")
            {
                (
                    format!("onboarding-health-{target_id}"),
                    format!("onboarding-inventory-{target_id}"),
                )
            } else if let Some(rotation_id) = deployment
                .idempotency_key
                .strip_prefix("agent-token-reconfiguration-")
            {
                (
                    format!("agent-token-health-{rotation_id}"),
                    format!("agent-token-inventory-{rotation_id}"),
                )
            } else {
                continue;
            };
        match ensure_deployment_followups(state, deployment, health_key, inventory_key).await {
            Ok(created) => queued += created,
            Err(error) => {
                tracing::warn!(job_id = %deployment.id.as_uuid(), error = ?error, "onboarding follow-up reconciliation failed")
            }
        }
    }
    for update in jobs.iter().filter(|job| {
        job.operation == AnsibleOperation::UpdatePackages
            && job.mode == ExecutionMode::Apply
            && job.status == AnsibleJobStatus::Succeeded
    }) {
        match queue_package_inventory_followup(state, update).await {
            Ok(created) => queued += created,
            Err(error) => {
                tracing::warn!(job_id = %update.id.as_uuid(), error = ?error, "package update inventory follow-up reconciliation failed")
            }
        }
    }
    Ok(queued)
}

async fn queue_package_inventory_followup(
    state: &ApiState,
    update: &AnsibleJob,
) -> Result<usize, ApiError> {
    let ResourceTarget::Target(target_id) = update.target else {
        return Ok(0);
    };
    let key = format!("package-update-inventory-{}", update.id.as_uuid());
    if find_existing_job(state, update.target, &key)
        .await?
        .is_some()
    {
        return Ok(0);
    }
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|item| item.id == target_id && item.state == TargetState::Managed)
        .cloned();
    let Some(target) = target else { return Ok(0) };
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::CollectPackageInventory,
            target: update.target,
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::CollectPackageInventory,
            secret_refs: vec![target.credential_secret_ref],
            idempotency_key: key,
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .map_err(map_ansible_error)?;
    if let JobSubmission::Created(job) = submission {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
        return Ok(1);
    }
    Ok(0)
}

pub(crate) async fn queue_agent_reconfiguration(
    state: &ApiState,
    target: &Target,
    idempotency_key: String,
) -> Result<Option<AnsibleJob>, ApiError> {
    if target.state == TargetState::Disabled {
        return Ok(None);
    }
    let resource_target = ResourceTarget::Target(target.id);
    if let Some(existing) = find_existing_job(state, resource_target, &idempotency_key).await? {
        return Ok(Some(existing));
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: resource_target,
            lifecycle: ResourceLifecycle::Pending,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
            idempotency_key,
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .map_err(map_ansible_error)?;
    let (job, created) = match submission {
        JobSubmission::Created(job) => (job, true),
        JobSubmission::Duplicate(job) => (job, false),
    };
    if created {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
    }
    Ok(Some(job))
}

async fn ensure_deployment_followups(
    state: &ApiState,
    deployment: &AnsibleJob,
    health_key: String,
    inventory_key: String,
) -> Result<usize, ApiError> {
    if deployment.status != AnsibleJobStatus::Succeeded {
        return Ok(0);
    }
    let ResourceTarget::Target(target_id) = deployment.target else {
        return Ok(0);
    };
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id && target.state == TargetState::Managed)
        .cloned();
    let Some(target) = target else { return Ok(0) };
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    let health = jobs
        .iter()
        .find(|job| job.idempotency_key == health_key)
        .cloned();
    let mut created = 0;
    if health.is_none() {
        let health = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::HealthCheck,
                target: ResourceTarget::Target(target_id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Check,
                parameters: AnsibleParameters::HealthCheck,
                secret_refs: Vec::new(),
                idempotency_key: health_key,
                confirmed: true,
                actor_role: ActorRole::Operator,
            })
            .map_err(map_ansible_error)?;
        if let JobSubmission::Created(job) = health {
            persist_created_job(state, &job).await?;
            state.publish(ApiEvent::status(
                "ansible_job",
                job.id.as_uuid().to_string(),
                "queued",
            ));
            created += 1;
        }
        return Ok(created);
    }
    if health
        .as_ref()
        .is_some_and(|job| job.status != AnsibleJobStatus::Succeeded)
    {
        return Ok(created);
    }
    if jobs.iter().any(|job| job.idempotency_key == inventory_key) {
        return Ok(created);
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::CollectPackageInventory,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::CollectPackageInventory,
            secret_refs: vec![target.credential_secret_ref],
            idempotency_key: inventory_key,
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .map_err(map_ansible_error)?;
    if let JobSubmission::Created(job) = submission {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
        created += 1;
    }
    Ok(created)
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

pub(super) async fn create_ansible_job(
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

async fn validate_package_update(
    state: &ApiState,
    request: &CreateAnsibleJobRequest,
    target: ResourceTarget,
) -> Result<(), ApiError> {
    if request.operation != AnsibleOperation::UpdatePackages {
        return Ok(());
    }
    let ResourceTarget::Target(target_id) = target else {
        return Err(ApiError::bad_request(
            "target_policy_required",
            "package updates require a registered target and update policy",
        ));
    };
    let target_kind = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .map(|target| target.kind)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    if target_kind == lxcup_core::TargetKind::WindowsServer {
        return Err(ApiError::bad_request(
            "package_update_platform_unsupported",
            "package-name update plans are currently supported on Debian and Ubuntu targets only",
        ));
    }
    let policy_id = request.policy_id.as_deref().ok_or_else(|| {
        ApiError::bad_request(
            "update_policy_required",
            "package updates require an explicit update policy",
        )
    })?;
    let policy = state
        .store
        .read()
        .await
        .update_policies
        .iter()
        .find(|policy| policy.id == policy_id && policy.enabled)
        .cloned()
        .ok_or_else(|| {
            ApiError::bad_request(
                "update_policy_unavailable",
                "the selected update policy is missing or disabled",
            )
        })?;
    let AnsibleParameters::UpdatePackages { packages } = &request.parameters else {
        return Err(ApiError::bad_request(
            "invalid_package_plan",
            "package update parameters are invalid",
        ));
    };
    // Until the package manager yields structured impact data, classify requested upgrades as high risk.
    if !policy.allowed_targets.contains(&target_id)
        || policy.maximum_risk < UpdateRisk::High
        || (!policy.allowed_packages.is_empty()
            && packages
                .iter()
                .any(|package| !policy.allowed_packages.contains(package)))
    {
        return Err(ApiError::forbidden(
            "update_policy_denied",
            "target, package, risk, or maintenance window is not allowed by the selected policy",
        ));
    }
    match request.mode {
        ExecutionMode::Plan
            if request.approved_plan_job_id.is_none()
                && request
                    .idempotency_key
                    .starts_with(&format!("package-plan:{policy_id}:")) =>
        {
            Ok(())
        }
        ExecutionMode::Apply => {
            let plan_id = request.approved_plan_job_id.ok_or_else(|| {
                ApiError::bad_request(
                    "package_plan_required",
                    "apply requires a completed package plan",
                )
            })?;
            if !request
                .idempotency_key
                .starts_with(&format!("package-apply:{}:", plan_id.as_uuid()))
            {
                return Err(ApiError::bad_request(
                    "invalid_package_apply_key",
                    "the apply request must be tied to its approved plan",
                ));
            }
            if packages.iter().any(|package| {
                !policy.allows(target_id, package, UpdateRisk::High, chrono::Utc::now())
            }) {
                return Err(ApiError::forbidden(
                    "update_policy_window_closed",
                    "package apply is only allowed inside the policy maintenance window",
                ));
            }
            if !request.confirmed {
                return Err(ApiError::bad_request(
                    "ansible_confirmation_required",
                    "applying a package plan requires explicit confirmation",
                ));
            }
            let plan = if let Some(repositories) = state.repositories.as_ref() {
                repositories
                    .ansible_jobs
                    .find_by_id(plan_id)
                    .await
                    .map_err(|_| ApiError::storage())?
                    .ok_or_else(|| ApiError::not_found("package plan not found"))?
            } else {
                state
                    .ansible
                    .read()
                    .await
                    .job(plan_id)
                    .map_err(map_ansible_error)?
            };
            if plan.operation != AnsibleOperation::UpdatePackages
                || plan.mode != ExecutionMode::Plan
                || plan.status != AnsibleJobStatus::Succeeded
                || plan.target != target
                || plan.parameters != request.parameters
                || !plan
                    .idempotency_key
                    .starts_with(&format!("package-plan:{policy_id}:"))
            {
                return Err(ApiError::conflict(
                    "package_plan_mismatch",
                    "the approved plan must be successful and match the exact target and package list",
                ));
            }
            let existing_jobs = if let Some(repositories) = state.repositories.as_ref() {
                repositories
                    .ansible_jobs
                    .list()
                    .await
                    .map_err(|_| ApiError::storage())?
            } else {
                state.ansible.read().await.jobs()
            };
            if existing_jobs.iter().any(|job| {
                job.operation == AnsibleOperation::UpdatePackages
                    && job.mode == ExecutionMode::Apply
                    && job
                        .idempotency_key
                        .starts_with(&format!("package-apply:{}:", plan_id.as_uuid()))
            }) {
                return Err(ApiError::conflict(
                    "package_plan_already_used",
                    "this plan already has an apply attempt; create a fresh plan before resuming",
                ));
            }
            Ok(())
        }
        _ => Err(ApiError::bad_request(
            "package_plan_mode_required",
            "package updates must first be planned; only a matching successful plan may be applied",
        )),
    }
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

#[cfg(test)]
mod package_policy_tests {
    use super::*;
    use lxcup_core::{TargetKind, TargetTransport, UpdatePolicy};

    fn package_request(
        mode: ExecutionMode,
        approved_plan_job_id: Option<lxcup_core::AnsibleJobId>,
        packages: &[&str],
    ) -> CreateAnsibleJobRequest {
        let idempotency_key = match (mode, approved_plan_job_id) {
            (ExecutionMode::Plan, _) => {
                format!("package-plan:security:{}", Uuid::new_v4())
            }
            (ExecutionMode::Apply, Some(plan_id)) => {
                format!("package-apply:{}:{}", plan_id.as_uuid(), Uuid::new_v4())
            }
            _ => Uuid::new_v4().to_string(),
        };
        CreateAnsibleJobRequest {
            operation: AnsibleOperation::UpdatePackages,
            target_id: Some(TargetId::new()),
            container_id: 0,
            mode,
            parameters: AnsibleParameters::UpdatePackages {
                packages: packages.iter().map(|item| (*item).to_owned()).collect(),
            },
            idempotency_key,
            confirmed: mode == ExecutionMode::Apply,
            policy_id: Some("security".to_owned()),
            approved_plan_job_id,
        }
    }

    #[tokio::test]
    async fn package_apply_requires_policy_and_a_matching_successful_plan() {
        let state = ApiState::new();
        let mut target = Target::new(
            "policy-target",
            TargetKind::LinuxServer,
            "192.0.2.1",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        {
            let mut store = state.store.write().await;
            store.update_policies.push(UpdatePolicy {
                id: "security".to_owned(),
                allowed_targets: vec![target.id],
                allowed_packages: vec!["curl".to_owned()],
                maintenance_start_minute: 0,
                maintenance_end_minute: 1439,
                timezone: "UTC".to_owned(),
                maximum_risk: UpdateRisk::High,
                enabled: true,
            });
            store.targets.push(target.clone());
        }
        let mut request = package_request(ExecutionMode::Plan, None, &["curl"]);
        request.target_id = Some(target.id);
        assert!(
            validate_package_update(&state, &request, ResourceTarget::Target(target.id))
                .await
                .is_ok()
        );
        request.parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["wget".to_owned()],
        };
        assert_eq!(
            validate_package_update(&state, &request, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "update_policy_denied"
        );

        let parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()],
        };
        let JobSubmission::Created(failed_plan) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Plan,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "package-plan:security:failed-plan".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("plan should be created")
        };
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(failed_plan.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(failed_plan.id, AnsibleJobStatus::Failed)
            .unwrap();
        drop(coordinator);
        let mut rejected_apply =
            package_request(ExecutionMode::Apply, Some(failed_plan.id), &["curl"]);
        rejected_apply.target_id = Some(target.id);
        assert_eq!(
            validate_package_update(&state, &rejected_apply, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "package_plan_mismatch"
        );

        let plan = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Plan,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "package-plan:security:approved-plan".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap();
        let JobSubmission::Created(plan) = plan else {
            panic!("plan should be created")
        };
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(plan.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Planned)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Applying)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Succeeded)
            .unwrap();
        drop(coordinator);
        let mut apply = package_request(ExecutionMode::Apply, Some(plan.id), &["curl"]);
        apply.target_id = Some(target.id);
        apply.confirmed = true;
        assert!(
            validate_package_update(&state, &apply, ResourceTarget::Target(target.id))
                .await
                .is_ok()
        );
        let JobSubmission::Created(applied_job) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Apply,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: apply.idempotency_key.clone(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("apply job should be created")
        };
        let mut coordinator = state.ansible.write().await;
        for status in [
            AnsibleJobStatus::Checking,
            AnsibleJobStatus::Planned,
            AnsibleJobStatus::Applying,
            AnsibleJobStatus::Failed,
        ] {
            coordinator.transition(applied_job.id, status).unwrap();
        }
        drop(coordinator);
        let retry = package_request(ExecutionMode::Apply, Some(plan.id), &["curl"]);
        assert_eq!(
            validate_package_update(&state, &retry, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "package_plan_already_used"
        );
        apply.parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["wget".to_owned()],
        };
        assert_eq!(
            validate_package_update(&state, &apply, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "update_policy_denied"
        );
    }

    #[tokio::test]
    async fn successful_package_apply_queues_one_inventory_refresh() {
        let state = ApiState::new();
        let mut target = Target::new(
            "updated-target",
            TargetKind::LinuxServer,
            "192.0.2.9",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        state.store.write().await.targets.push(target.clone());
        let JobSubmission::Created(job) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Apply,
                parameters: AnsibleParameters::UpdatePackages {
                    packages: vec!["curl".to_owned()],
                },
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "completed-package-update".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("package job should be created")
        };
        let mut coordinator = state.ansible.write().await;
        for status in [
            AnsibleJobStatus::Checking,
            AnsibleJobStatus::Planned,
            AnsibleJobStatus::Applying,
            AnsibleJobStatus::Succeeded,
        ] {
            coordinator.transition(job.id, status).unwrap();
        }
        drop(coordinator);

        assert_eq!(reconcile_onboarding_jobs(&state).await.unwrap(), 1);
        assert_eq!(reconcile_onboarding_jobs(&state).await.unwrap(), 0);
        let jobs = state.ansible.read().await.jobs();
        assert!(jobs.iter().any(|item| {
            item.operation == AnsibleOperation::CollectPackageInventory
                && item.idempotency_key == format!("package-update-inventory-{}", job.id.as_uuid())
        }));
    }
}

pub(super) async fn persist_created_job(
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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RetryAnsibleJobRequest {
    #[serde(default)]
    confirmed: bool,
}

pub(super) async fn retry_ansible_job(
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
