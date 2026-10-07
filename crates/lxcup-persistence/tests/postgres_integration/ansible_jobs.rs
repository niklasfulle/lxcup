use super::*;

#[tokio::test]
async fn postgres_ansible_job_repository_claims_jobs_and_persists_events() {
    let Some(test_database) = scoped_test_database("ansible_jobs").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let target = Target::new(
        "ansible-job-repository-target",
        TargetKind::LinuxServer,
        "192.0.2.55",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let target_ref = ResourceTarget::Target(target.id);
    let job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "repository-job-test".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap();
    let job_id = job.id;
    repositories.ansible_jobs.save(&job).await.unwrap();
    let queue_metrics = repositories.ansible_jobs.queue_metrics().await.unwrap();
    assert!(queue_metrics.queued_jobs >= 1);
    assert!(queue_metrics.oldest_queued_at.is_some());
    repositories
        .worker_heartbeats
        .record("integration-worker", "0.4.0", false, chrono::Utc::now())
        .await
        .unwrap();
    let heartbeat_status = repositories
        .worker_heartbeats
        .latest_status()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(heartbeat_status.artifact_store_available, Some(false));
    assert!(heartbeat_status.artifact_store_checked_at.is_some());
    assert!(
        repositories
            .worker_heartbeats
            .latest()
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        repositories
            .ansible_jobs
            .find_by_id(job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Queued
    );
    assert!(
        repositories
            .ansible_jobs
            .find_by_idempotency_key(target_ref, "repository-job-test")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert_eq!(
        repositories
            .ansible_jobs
            .list()
            .await
            .unwrap()
            .iter()
            .filter(|item| item.id == job_id)
            .count(),
        1
    );

    let claimed = repositories
        .ansible_jobs
        .claim_next_queued()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, job_id);
    assert_eq!(claimed.status, AnsibleJobStatus::Checking);
    assert!(
        repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert!(
        repositories
            .ansible_jobs
            .claim_next_queued()
            .await
            .unwrap()
            .is_none(),
    );
    let events = repositories.ansible_jobs.events(job_id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].event,
        JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Checking
        }
    ));

    let event = JobEvent {
        sequence: 2,
        job_id,
        event: JobEventKind::WorkerLog {
            source: "stdout".to_owned(),
            message: "health ok".to_owned(),
        },
        created_at: Utc::now(),
    };
    repositories
        .ansible_jobs
        .append_event(&event)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .ansible_jobs
            .events(job_id)
            .await
            .unwrap()
            .len(),
        2
    );

    let mut finished = claimed;
    finished.transition_to(AnsibleJobStatus::Planned).unwrap();
    finished.transition_to(AnsibleJobStatus::Failed).unwrap();
    repositories.ansible_jobs.update(&finished).await.unwrap();
    assert_eq!(
        repositories
            .ansible_jobs
            .find_by_id(job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Failed
    );
    let retried = repositories.ansible_jobs.retry(job_id).await.unwrap();
    assert_eq!(retried.status, AnsibleJobStatus::Queued);
    assert!(repositories.ansible_jobs.retry(job_id).await.is_err());
    let retry_events = repositories.ansible_jobs.events(job_id).await.unwrap();
    assert_eq!(retry_events.len(), 3);
    assert!(matches!(
        retry_events.last().map(|event| &event.event),
        Some(JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Queued
        })
    ));
    let recent_jobs = repositories
        .ansible_jobs
        .list_since(Utc::now() - chrono::Duration::minutes(1), 1)
        .await
        .unwrap();
    assert_eq!(recent_jobs.len(), 1);
    assert_eq!(recent_jobs[0].id, job_id);
    let recent_events = repositories
        .ansible_jobs
        .events_since(job_id, Utc::now() - chrono::Duration::minutes(1), 1)
        .await
        .unwrap();
    assert_eq!(recent_events.len(), 1);
    assert!(matches!(
        recent_events[0].event,
        JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Queued
        }
    ));
    assert!(
        !repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    assert!(
        repositories
            .ansible_jobs
            .find_by_idempotency_key(target_ref, "missing-key")
            .await
            .unwrap()
            .is_none()
    );

    let pool = database.pool();
    sqlx::query("DELETE FROM ansible_job_events WHERE job_id = $1")
        .bind(job_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ansible_jobs WHERE id = $1")
        .bind(job_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM targets WHERE id = $1")
        .bind(target.id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    test_database.finish().await;
}

#[tokio::test]
async fn postgres_worker_claims_reconciliation_while_unresolved_apply_locks_target() {
    let Some(test_database) = scoped_test_database("ansible_reconciliation").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let target = Target::new(
        "ansible-reconcile-repository-target",
        TargetKind::LinuxServer,
        "192.0.2.56",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let target_ref = ResourceTarget::Target(target.id);
    let mut source = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::RepairAgent,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Apply,
        parameters: AnsibleParameters::RepairAgent,
        secret_refs: vec![
            target
                .credential_secret_ref
                .expect("test target has credentials"),
            target.agent_secret_ref,
        ],
        idempotency_key: "reconcile-source-job".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::ReconcileRequired,
    ] {
        source.transition_to(status).unwrap();
    }
    repositories.ansible_jobs.save(&source).await.unwrap();
    assert!(
        repositories
            .ansible_jobs
            .has_reconciliation_required_target(target_ref)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .ansible_jobs
            .has_other_active_target_job(target_ref, source.id)
            .await
            .unwrap()
    );

    let queued_healthcheck = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "queued-healthcheck-before-reconcile".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories
        .ansible_jobs
        .save(&queued_healthcheck)
        .await
        .unwrap();

    let mut coordinator = lxcup_ansible::AnsibleJobCoordinator::default();
    let lxcup_ansible::JobSubmission::Created(reconciliation) =
        coordinator.submit_reconciliation(&source).unwrap()
    else {
        panic!("reconciliation should be newly queued")
    };
    repositories
        .ansible_jobs
        .save(&reconciliation)
        .await
        .unwrap();
    assert!(
        repositories
            .ansible_jobs
            .has_other_active_target_job(target_ref, reconciliation.id)
            .await
            .unwrap()
    );

    assert!(
        repositories
            .ansible_jobs
            .has_active_target(target_ref)
            .await
            .unwrap()
    );
    let claimed = repositories
        .ansible_jobs
        .claim_next_queued()
        .await
        .unwrap()
        .expect("the linked reconciliation must bypass its source lock");
    assert_eq!(claimed.id, reconciliation.id);
    assert_eq!(claimed.mode, ExecutionMode::Reconcile);
    assert_eq!(claimed.status, AnsibleJobStatus::Checking);

    test_database.finish().await;
}

#[tokio::test]
async fn postgres_windows_agent_claims_only_allowlisted_jobs_and_persists_apply_transitions() {
    let Some(test_database) = scoped_test_database("windows_agent_jobs").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let target = Target::new_agent_only(
        "windows-agent-queue-target",
        TargetKind::WindowsServer,
        "192.0.2.60",
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let target_ref = ResourceTarget::Target(target.id);
    let plan = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::UpdatePackages,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Plan,
        parameters: AnsibleParameters::UpdatePackages {
            packages: vec!["7zip.7zip".to_owned()],
        },
        secret_refs: vec![target.agent_secret_ref],
        idempotency_key: "windows-agent-plan".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories.ansible_jobs.save(&plan).await.unwrap();
    assert!(
        repositories
            .ansible_jobs
            .claim_next_queued()
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repositories
            .ansible_jobs
            .claim_next_windows_agent_job(lxcup_core::TargetId::new())
            .await
            .unwrap()
            .is_none()
    );

    let claimed_plan = repositories
        .ansible_jobs
        .claim_next_windows_agent_job(target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed_plan.id, plan.id);
    assert_eq!(claimed_plan.status, AnsibleJobStatus::Checking);
    let mut completed_plan = claimed_plan;
    completed_plan
        .transition_to(AnsibleJobStatus::Planned)
        .unwrap();
    completed_plan
        .transition_to(AnsibleJobStatus::Succeeded)
        .unwrap();
    repositories
        .ansible_jobs
        .update(&completed_plan)
        .await
        .unwrap();

    let inventory = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::CollectPackageInventory,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::CollectPackageInventory,
        secret_refs: vec![target.agent_secret_ref],
        idempotency_key: "windows-agent-inventory".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories.ansible_jobs.save(&inventory).await.unwrap();
    assert!(
        repositories
            .ansible_jobs
            .claim_next_queued()
            .await
            .unwrap()
            .is_none(),
        "Windows package inventory must never be claimed by the Ansible worker"
    );
    let claimed_inventory = repositories
        .ansible_jobs
        .claim_next_windows_agent_job(target.id)
        .await
        .unwrap()
        .expect("the Windows agent should claim package inventory");
    assert_eq!(claimed_inventory.id, inventory.id);
    assert_eq!(claimed_inventory.status, AnsibleJobStatus::Checking);
    let mut completed_inventory = claimed_inventory;
    completed_inventory
        .transition_to(AnsibleJobStatus::Planned)
        .unwrap();
    completed_inventory
        .transition_to(AnsibleJobStatus::Succeeded)
        .unwrap();
    repositories
        .ansible_jobs
        .update(&completed_inventory)
        .await
        .unwrap();

    let apply = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::UpdatePackages,
        target: target_ref,
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Apply,
        parameters: AnsibleParameters::UpdatePackages {
            packages: vec!["7zip.7zip".to_owned()],
        },
        secret_refs: vec![target.agent_secret_ref],
        idempotency_key: "windows-agent-apply".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories.ansible_jobs.save(&apply).await.unwrap();
    let claimed_apply = repositories
        .ansible_jobs
        .claim_next_windows_agent_job(target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed_apply.status, AnsibleJobStatus::Applying);
    assert_eq!(
        repositories
            .ansible_jobs
            .events(apply.id)
            .await
            .unwrap()
            .len(),
        3
    );

    let unsupported_target = Target::new_agent_only(
        "windows-agent-unsupported-queue-target",
        TargetKind::WindowsServer,
        "192.0.2.61",
        SecretId::new(),
    )
    .unwrap();
    repositories
        .targets
        .save(&unsupported_target)
        .await
        .unwrap();
    let unsupported = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::DeployAgent,
        target: ResourceTarget::Target(unsupported_target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Plan,
        parameters: AnsibleParameters::DeployAgent {
            agent_version: "0.6.0".to_owned(),
        },
        secret_refs: vec![unsupported_target.agent_secret_ref],
        idempotency_key: "windows-agent-deploy-unsupported".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories.ansible_jobs.save(&unsupported).await.unwrap();
    assert!(
        repositories
            .ansible_jobs
            .claim_next_windows_agent_job(unsupported_target.id)
            .await
            .unwrap()
            .is_none()
    );

    test_database.finish().await;
}
