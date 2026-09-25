use super::*;

#[tokio::test]
async fn postgres_ansible_job_repository_claims_jobs_and_persists_events() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let config = DatabaseConfig::from_values(
        database_url,
        3,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = Repositories::new(&database);
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
        .record("integration-worker")
        .await
        .unwrap();
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
    assert_eq!(
        repositories
            .ansible_jobs
            .claim_next_queued()
            .await
            .unwrap()
            .is_none(),
        true
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
}
