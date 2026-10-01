use super::*;
use lxcup_core::{ContainerId, SecretId};

fn request(operation: AnsibleOperation, parameters: AnsibleParameters) -> AnsibleJobRequest {
    AnsibleJobRequest {
        operation,
        target: ResourceTarget::Container(ContainerId::new(101)),
        lifecycle: ResourceLifecycle::Managed,
        mode: if matches!(
            operation,
            AnsibleOperation::HealthCheck | AnsibleOperation::CollectPackageInventory
        ) {
            ExecutionMode::Check
        } else {
            ExecutionMode::Apply
        },
        parameters,
        secret_refs: vec![SecretId::new()],
        idempotency_key: "job-1".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    }
}

#[test]
fn registry_resolves_only_versioned_allowlisted_playbooks() {
    let spec = PlaybookRegistry.resolve(AnsibleOperation::DeployAgent);

    assert_eq!(spec.playbook, "agent/deploy.yml");
    assert_eq!(spec.version, "1");
    assert!(spec.requires_secret);
    let inventory = PlaybookRegistry.resolve(AnsibleOperation::CollectPackageInventory);
    assert_eq!(inventory.playbook, "inventory/packages.yml");
    assert_eq!(inventory.version, "1");
    assert!(inventory.requires_secret);
}

#[test]
fn operation_mode_matrix_covers_every_operation_and_rejects_reconcile() {
    let check_plan_apply = vec![
        ExecutionMode::Check,
        ExecutionMode::Plan,
        ExecutionMode::Apply,
    ];
    let expected = vec![
        (AnsibleOperation::DeployAgent, check_plan_apply.clone()),
        (AnsibleOperation::UpdateAgent, check_plan_apply.clone()),
        (
            AnsibleOperation::UpdatePackages,
            vec![ExecutionMode::Plan, ExecutionMode::Apply],
        ),
        (AnsibleOperation::RepairAgent, check_plan_apply.clone()),
        (
            AnsibleOperation::ConfigureTarget,
            vec![ExecutionMode::Check, ExecutionMode::Apply],
        ),
        (AnsibleOperation::HealthCheck, vec![ExecutionMode::Check]),
        (
            AnsibleOperation::CollectPackageInventory,
            vec![ExecutionMode::Check],
        ),
    ];
    let modes = [
        ExecutionMode::Check,
        ExecutionMode::Plan,
        ExecutionMode::Apply,
        ExecutionMode::Reconcile,
    ];
    for (operation, expected_modes) in expected {
        assert_eq!(operation.supported_modes(), expected_modes);
        for mode in modes {
            assert_eq!(
                operation.supports_mode(mode),
                expected_modes.contains(&mode),
                "matrix mismatch for {operation:?}/{mode:?}"
            );
            assert!(!operation.supports_mode(ExecutionMode::Reconcile));
        }
    }
}

#[test]
fn job_contract_rejects_a_mode_outside_the_shared_matrix() {
    let mut invalid = request(
        AnsibleOperation::HealthCheck,
        AnsibleParameters::HealthCheck,
    );
    invalid.mode = ExecutionMode::Apply;
    assert_eq!(
        AnsibleJob::from_request(invalid),
        Err(AnsibleContractError::UnsupportedMode)
    );
}

#[test]
fn every_mutating_apply_requires_explicit_confirmation_but_checks_do_not() {
    let requests = [
        (
            AnsibleOperation::DeployAgent,
            AnsibleParameters::DeployAgent {
                agent_version: "0.3.1".to_owned(),
            },
        ),
        (
            AnsibleOperation::UpdateAgent,
            AnsibleParameters::UpdateAgent {
                agent_version: "0.3.1".to_owned(),
            },
        ),
        (
            AnsibleOperation::UpdatePackages,
            AnsibleParameters::UpdatePackages {
                packages: vec!["curl".to_owned()],
            },
        ),
        (
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ),
        (
            AnsibleOperation::ConfigureTarget,
            AnsibleParameters::ConfigureTarget {
                desired_hostname: Some("managed-host".to_owned()),
            },
        ),
    ];
    for (operation, parameters) in requests {
        let mut unconfirmed = request(operation, parameters);
        unconfirmed.confirmed = false;
        assert_eq!(
            AnsibleJob::from_request(unconfirmed),
            Err(AnsibleContractError::ConfirmationRequired),
            "unconfirmed apply was accepted for {operation:?}"
        );
    }

    let mut read_only = request(
        AnsibleOperation::HealthCheck,
        AnsibleParameters::HealthCheck,
    );
    read_only.confirmed = false;
    assert!(AnsibleJob::from_request(read_only).is_ok());
}

#[test]
fn all_packages_selector_must_be_used_alone() {
    let mut all = request(
        AnsibleOperation::UpdatePackages,
        AnsibleParameters::UpdatePackages {
            packages: vec!["*".to_owned()],
        },
    );
    assert!(AnsibleJob::from_request(all.clone()).is_ok());

    all.parameters = AnsibleParameters::UpdatePackages {
        packages: vec!["curl".to_owned(), "*".to_owned()],
    };
    assert_eq!(
        AnsibleJob::from_request(all),
        Err(AnsibleContractError::Invalid)
    );
}

#[test]
fn reconciliation_is_linked_to_one_unresolved_apply_and_idempotent() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let source = match coordinator
        .submit(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::ReconcileRequired,
    ] {
        coordinator.transition(source.id, status).unwrap();
    }

    let current_source = coordinator.job(source.id).unwrap();
    let JobSubmission::Created(reconcile) =
        coordinator.submit_reconciliation(&current_source).unwrap()
    else {
        panic!("first reconciliation must create a job")
    };
    assert_eq!(reconcile.mode, ExecutionMode::Reconcile);
    assert_eq!(reconcile.operation, source.operation);
    assert_eq!(reconcile.target, source.target);
    assert_eq!(
        reconciliation_source_job_id(&reconcile.idempotency_key),
        Some(source.id)
    );
    let current_source = coordinator.job(source.id).unwrap();
    assert_eq!(
        coordinator.submit_reconciliation(&current_source).unwrap(),
        JobSubmission::Duplicate(reconcile)
    );
    let mut resolved_source = current_source;
    resolved_source.status = AnsibleJobStatus::Succeeded;
    assert_eq!(
        coordinator.submit_reconciliation(&resolved_source),
        Err(CoordinatorError::ReconcileNotAllowed)
    );
}

#[test]
fn reconciliation_rejects_jobs_without_unresolved_apply_state() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let source = match coordinator
        .submit(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    assert_eq!(
        coordinator.submit_reconciliation(&source),
        Err(CoordinatorError::ReconcileNotAllowed)
    );
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::Succeeded,
    ] {
        coordinator.transition(source.id, status).unwrap();
    }
    let succeeded = coordinator.job(source.id).unwrap();
    assert_eq!(
        coordinator.submit_reconciliation(&succeeded),
        Err(CoordinatorError::ReconcileNotAllowed)
    );
    let mut aborted = succeeded;
    aborted.status = AnsibleJobStatus::Aborted;
    assert_eq!(
        coordinator.submit_reconciliation(&aborted),
        Err(CoordinatorError::ReconcileNotAllowed)
    );
}

#[test]
fn persisted_reconciliation_can_bypass_a_stale_in_memory_target_lock() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let blocker = match coordinator
        .submit(request(
            AnsibleOperation::HealthCheck,
            AnsibleParameters::HealthCheck,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    coordinator
        .transition(blocker.id, AnsibleJobStatus::Checking)
        .unwrap();

    let mut source_request = request(
        AnsibleOperation::RepairAgent,
        AnsibleParameters::RepairAgent,
    );
    source_request.target = blocker.target;
    let mut source = AnsibleJob::from_request(source_request).unwrap();
    source.status = AnsibleJobStatus::ReconcileRequired;

    assert_eq!(
        coordinator.submit_reconciliation(&source),
        Err(CoordinatorError::TargetBusy)
    );
    assert!(matches!(
        coordinator.submit_persisted_reconciliation(&source),
        Ok(JobSubmission::Created(_))
    ));
}

#[test]
fn job_contains_hash_and_never_secret_values() {
    let job = AnsibleJob::from_request(request(
        AnsibleOperation::UpdatePackages,
        AnsibleParameters::UpdatePackages {
            packages: vec!["nginx".to_owned()],
        },
    ))
    .unwrap();
    let json = serde_json::to_string(&job).unwrap();

    assert_eq!(job.playbook, "packages/update.yml");
    assert_eq!(job.playbook_version, "1");
    assert_eq!(job.parameter_hash.len(), 64);
    assert!(json.contains("secret_refs"));
    assert!(!json.contains("top-secret"));
}

#[test]
fn typed_parameters_and_policy_are_enforced() {
    let mut invalid = request(
        AnsibleOperation::UpdateAgent,
        AnsibleParameters::DeployAgent {
            agent_version: "0.3.1".to_owned(),
        },
    );
    assert_eq!(
        AnsibleJob::from_request(invalid.clone()),
        Err(AnsibleContractError::Invalid)
    );

    invalid = request(
        AnsibleOperation::DeployAgent,
        AnsibleParameters::DeployAgent {
            agent_version: "0.3.1".to_owned(),
        },
    );
    invalid.actor_role = ActorRole::Viewer;
    assert_eq!(
        AnsibleJob::from_request(invalid),
        Err(AnsibleContractError::PermissionDenied)
    );

    let mut missing_secret = request(
        AnsibleOperation::DeployAgent,
        AnsibleParameters::DeployAgent {
            agent_version: "0.3.1".to_owned(),
        },
    );
    missing_secret.secret_refs.clear();
    assert_eq!(
        AnsibleJob::from_request(missing_secret),
        Err(AnsibleContractError::MissingSecretReference)
    );
}

#[test]
fn job_transitions_require_worker_reconciliation_after_apply_loss() {
    let mut job = AnsibleJob::from_request(request(
        AnsibleOperation::RepairAgent,
        AnsibleParameters::RepairAgent,
    ))
    .unwrap();
    job.transition_to(AnsibleJobStatus::Checking).unwrap();
    job.transition_to(AnsibleJobStatus::Planned).unwrap();
    job.transition_to(AnsibleJobStatus::Applying).unwrap();
    job.transition_to(AnsibleJobStatus::ReconcileRequired)
        .unwrap();
    assert!(job.transition_to(AnsibleJobStatus::Succeeded).is_ok());
    assert_eq!(
        job.transition_to(AnsibleJobStatus::Applying),
        Err(AnsibleContractError::InvalidTransition)
    );
}

#[test]
fn check_jobs_succeed_directly_from_planned_without_applying() {
    let mut job = AnsibleJob::from_request(request(
        AnsibleOperation::HealthCheck,
        AnsibleParameters::HealthCheck,
    ))
    .unwrap();
    assert_eq!(job.mode, ExecutionMode::Check);
    job.transition_to(AnsibleJobStatus::Checking).unwrap();
    job.transition_to(AnsibleJobStatus::Planned).unwrap();
    job.transition_to(AnsibleJobStatus::Succeeded).unwrap();
    assert_eq!(job.status, AnsibleJobStatus::Succeeded);
    assert_eq!(
        job.transition_to(AnsibleJobStatus::Applying),
        Err(AnsibleContractError::InvalidTransition)
    );
}

#[test]
fn coordinator_allows_queued_jobs_but_serializes_execution() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let first = match coordinator
        .submit(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => panic!("first submission must create a job"),
    };

    let duplicate = coordinator
        .submit(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap();
    assert_eq!(duplicate, JobSubmission::Duplicate(first.clone()));

    let mut conflicting_request = request(
        AnsibleOperation::HealthCheck,
        AnsibleParameters::HealthCheck,
    );
    conflicting_request.idempotency_key = "different".to_owned();
    let second = match coordinator.submit(conflicting_request).unwrap() {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => panic!("different key must create a queued job"),
    };
    coordinator
        .transition(first.id, AnsibleJobStatus::Checking)
        .unwrap();
    assert_eq!(
        coordinator.transition(second.id, AnsibleJobStatus::Checking),
        Err(CoordinatorError::TargetBusy)
    );
}

#[test]
fn coordinator_retries_safe_jobs_but_not_package_apply() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let repair = match coordinator
        .submit(request(
            AnsibleOperation::RepairAgent,
            AnsibleParameters::RepairAgent,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    coordinator
        .transition(repair.id, AnsibleJobStatus::Checking)
        .unwrap();
    coordinator
        .transition(repair.id, AnsibleJobStatus::Planned)
        .unwrap();
    coordinator
        .transition(repair.id, AnsibleJobStatus::Applying)
        .unwrap();
    coordinator
        .transition(repair.id, AnsibleJobStatus::Failed)
        .unwrap();
    assert_eq!(
        coordinator.retry(repair.id).unwrap().status,
        AnsibleJobStatus::Queued
    );

    let mut package_request = request(
        AnsibleOperation::UpdatePackages,
        AnsibleParameters::UpdatePackages {
            packages: vec!["nginx".to_owned()],
        },
    );
    package_request.idempotency_key = "package-1".to_owned();
    package_request.target = ResourceTarget::Container(lxcup_core::ContainerId::new(102));
    let package = match coordinator.submit(package_request).unwrap() {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    coordinator
        .transition(package.id, AnsibleJobStatus::Checking)
        .unwrap();
    coordinator
        .transition(package.id, AnsibleJobStatus::Planned)
        .unwrap();
    coordinator
        .transition(package.id, AnsibleJobStatus::Applying)
        .unwrap();
    coordinator
        .transition(package.id, AnsibleJobStatus::Failed)
        .unwrap();
    assert_eq!(
        coordinator.retry(package.id),
        Err(CoordinatorError::RetryNotAllowed)
    );
}

#[test]
fn coordinator_events_are_ordered_and_audit_safe() {
    let mut coordinator = AnsibleJobCoordinator::default();
    let job = match coordinator
        .submit(request(
            AnsibleOperation::HealthCheck,
            AnsibleParameters::HealthCheck,
        ))
        .unwrap()
    {
        JobSubmission::Created(job) => job,
        JobSubmission::Duplicate(_) => unreachable!(),
    };
    coordinator
        .record_event(
            job.id,
            JobEventKind::TaskStarted {
                task: "health_check".to_owned(),
            },
        )
        .unwrap();
    let events = coordinator.events(job.id).unwrap();
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert!(!serde_json::to_string(&events).unwrap().contains("stdout"));
}
