use super::*;
use crate::{ContainerId, Enrollment, Target};
use lxcup_ansible::{AnsibleContractError, CoordinatorError};
use lxcup_core::{TargetKind, TargetTransport};
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

#[tokio::test]
async fn job_dto_exposes_package_scope_and_only_valid_plan_references() {
    let state = ApiState::new();
    let mut request = test_request("package-plan:safe-policy:target");
    request.operation = AnsibleOperation::UpdatePackages;
    request.mode = ExecutionMode::Plan;
    request.parameters = AnsibleParameters::UpdatePackages {
        packages: vec!["curl".to_owned()],
    };
    request.secret_refs = vec![SecretId::new()];
    let JobSubmission::Created(mut job) = state.ansible.write().await.submit(request).unwrap()
    else {
        panic!("the package plan should be newly queued")
    };
    let dto = AnsibleJobDto::from(&job);
    assert_eq!(
        dto.package_names.as_deref(),
        Some(["curl".to_owned()].as_slice())
    );
    assert_eq!(dto.update_policy_id.as_deref(), Some("safe-policy"));
    assert!(dto.approved_plan_job_id.is_none());

    job.idempotency_key = "package-apply:not-a-uuid:safe-policy".to_owned();
    assert!(AnsibleJobDto::from(&job).approved_plan_job_id.is_none());
    job.idempotency_key = format!("package-apply:{}:safe-policy", Uuid::new_v4());
    assert!(AnsibleJobDto::from(&job).approved_plan_job_id.is_some());
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
async fn windows_workflows_require_the_local_agent_transport() {
    let state = ApiState::new();
    let target = Target::new(
        "legacy-windows",
        TargetKind::WindowsServer,
        "192.0.2.90",
        TargetTransport::Winrm,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    state.store.write().await.targets.push(target);

    let error = validate_windows_agent_operation(
        &state,
        ResourceTarget::Target(target_id),
        AnsibleOperation::HealthCheck,
        &AnsibleParameters::HealthCheck,
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, "windows_local_setup_required");
}

#[tokio::test]
async fn windows_agent_update_must_target_the_current_controller_version() {
    let state = ApiState::new();
    let target = Target::new_agent_only(
        "windows-update-target",
        TargetKind::WindowsServer,
        "192.0.2.91",
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    state.store.write().await.targets.push(target);

    let accepted = validate_windows_agent_operation(
        &state,
        ResourceTarget::Target(target_id),
        AnsibleOperation::UpdateAgent,
        &AnsibleParameters::UpdateAgent {
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )
    .await;
    assert!(accepted.is_ok());

    let rejected = validate_windows_agent_operation(
        &state,
        ResourceTarget::Target(target_id),
        AnsibleOperation::UpdateAgent,
        &AnsibleParameters::UpdateAgent {
            agent_version: "0.4.0".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.code, "windows_agent_version_unsupported");
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
    let events = get_ansible_job_events(State(state.clone()), Path(job.id.as_uuid().to_string()))
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
        .transition(job.id, AnsibleJobStatus::Planned)
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
        get_ansible_job(State(state), Path("not-a-uuid".to_owned()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mutating_apply_retry_requires_permission_and_explicit_confirmation() {
    let state = ApiState::new();
    let request = AnsibleJobRequest {
        operation: AnsibleOperation::DeployAgent,
        target: ResourceTarget::Target(TargetId::new()),
        lifecycle: ResourceLifecycle::Pending,
        mode: ExecutionMode::Apply,
        parameters: AnsibleParameters::DeployAgent {
            agent_version: "0.4.0".to_owned(),
        },
        secret_refs: vec![SecretId::new(), SecretId::new()],
        idempotency_key: "retry-confirmation-required".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    };
    let JobSubmission::Created(job) = state.ansible.write().await.submit(request).unwrap() else {
        panic!("the deployment should create a retryable test job")
    };
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
        .transition(job.id, AnsibleJobStatus::Planned)
        .unwrap();
    state
        .ansible
        .write()
        .await
        .transition(job.id, AnsibleJobStatus::Applying)
        .unwrap();
    state
        .ansible
        .write()
        .await
        .transition(job.id, AnsibleJobStatus::Failed)
        .unwrap();

    let denied = retry_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Viewer),
        Path(job.id.as_uuid().to_string()),
        JsonBody(RetryAnsibleJobRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let unconfirmed = retry_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Operator),
        Path(job.id.as_uuid().to_string()),
        JsonBody(RetryAnsibleJobRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(unconfirmed.code, "ansible_confirmation_required");
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::Failed
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

#[tokio::test]
async fn active_target_job_lookup_reports_the_lock_owner_and_honors_exclusion() {
    let state = ApiState::new();
    let request = test_request("active-lock-owner");
    let target = request.target;
    let JobSubmission::Created(job) = state.ansible.write().await.submit(request).unwrap() else {
        panic!("the lock owner must be a new job")
    };
    state
        .ansible
        .write()
        .await
        .transition(job.id, AnsibleJobStatus::Checking)
        .unwrap();

    let blocking = find_active_target_job(&state, target, None)
        .await
        .unwrap()
        .expect("active job must be discoverable");
    assert_eq!(blocking.id, job.id);
    assert_eq!(job_status_name(blocking.status), "checking");
    assert!(
        find_active_target_job(&state, target, Some(job.id))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn create_workflow_is_idempotent_and_returns_the_active_lock_owner() {
    let state = ApiState::new();
    let mut target = Target::new(
        "workflow-lock-target",
        lxcup_core::TargetKind::LinuxServer,
        "192.0.2.51",
        lxcup_core::TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    state.store.write().await.targets.push(target);
    let create = |idempotency_key: &str| CreateAnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target_id: Some(target_id),
        container_id: 0,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        idempotency_key: idempotency_key.to_owned(),
        confirmed: true,
        policy_id: None,
        approved_plan_job_id: None,
    };

    let first = create_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Operator),
        JsonBody(create("lock-owner")),
    )
    .await
    .unwrap();
    assert_eq!(first.0, StatusCode::ACCEPTED);
    let owner_id = first.1.0.data.id;
    let duplicate = create_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Operator),
        JsonBody(create("lock-owner")),
    )
    .await
    .unwrap();
    assert_eq!(duplicate.0, StatusCode::OK);
    assert_eq!(duplicate.1.0.data.id, owner_id);
    assert_eq!(state.ansible.read().await.jobs().len(), 1);

    state
        .ansible
        .write()
        .await
        .transition(owner_id, AnsibleJobStatus::Checking)
        .unwrap();
    let conflict = create_ansible_job(
        State(state.clone()),
        Extension(ActorRole::Operator),
        JsonBody(create("competing-workflow")),
    )
    .await
    .unwrap_err();
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    let blocker = conflict.blocking_job.expect("lock owner must be exposed");
    assert_eq!(blocker.id, owner_id.as_uuid());
    assert_eq!(blocker.status, "checking");
    assert_eq!(state.ansible.read().await.jobs().len(), 1);
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn unsafe_env_remove() {
    unsafe { std::env::remove_var("LXCUP_ANSIBLE_SECRET_IDS") };
}
