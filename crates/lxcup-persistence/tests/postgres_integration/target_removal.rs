use super::*;

#[tokio::test]
async fn deleting_target_removes_scoped_data_and_prunes_schedules_and_policies() {
    let Some(test_database) = scoped_test_database("target_remove_scoped").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let suffix = Uuid::new_v4().simple().to_string();
    let target = Target::new(
        format!("delete-target-{suffix}"),
        TargetKind::Lxc,
        "192.0.2.90",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let other_target = lxcup_core::TargetId::new();
    repositories.targets.save(&target).await.unwrap();

    let now = postgres_now();
    let pool = database.pool();
    sqlx::query("INSERT INTO package_inventory_snapshots (target_id, collected_at, status, package_count) VALUES ($1, $2, 'complete', 1)")
        .bind(target_id.as_uuid()).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO package_inventory_packages (target_id, package_name, installed_version) VALUES ($1, 'curl', '1.0')")
        .bind(target_id.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO target_telemetry_samples (target_id, collected_at, payload) VALUES ($1, $2, '{}'::jsonb)")
        .bind(target_id.as_uuid()).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO target_docker_inventory (target_id, collected_at, containers) VALUES ($1, $2, '[]'::jsonb)")
        .bind(target_id.as_uuid()).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO target_docker_telemetry_samples (target_id, container_id, collected_at, payload) VALUES ($1, 'abcdef123456', $2, '{}'::jsonb)")
        .bind(target_id.as_uuid()).bind(now).execute(pool).await.unwrap();

    let shared_policy = UpdatePolicy {
        id: format!("shared-delete-policy-{suffix}"),
        allowed_targets: vec![target_id, other_target],
        allowed_packages: vec![],
        maintenance_start_minute: 0,
        maintenance_end_minute: 1439,
        timezone: "UTC".to_owned(),
        maximum_risk: UpdateRisk::High,
        enabled: true,
    };
    let exclusive_policy = UpdatePolicy {
        id: format!("exclusive-delete-policy-{suffix}"),
        allowed_targets: vec![target_id],
        ..shared_policy.clone()
    };
    repositories
        .update_policies
        .save(&shared_policy)
        .await
        .unwrap();
    repositories
        .update_policies
        .save(&exclusive_policy)
        .await
        .unwrap();
    let shared_schedule = format!("shared-delete-schedule-{suffix}");
    let exclusive_schedule = format!("exclusive-delete-schedule-{suffix}");
    for (schedule_id, target_ids, policy_id) in [
        (
            shared_schedule.as_str(),
            vec![target_id.as_uuid(), other_target.as_uuid()],
            Some(shared_policy.id.as_str()),
        ),
        (
            exclusive_schedule.as_str(),
            vec![target_id.as_uuid()],
            Some(exclusive_policy.id.as_str()),
        ),
    ] {
        sqlx::query("INSERT INTO schedules (id, operation, timezone, target_ids, every_minutes, policy_id, next_run_at) VALUES ($1, 'collect_package_inventory', 'UTC', $2, 60, $3, $4)")
            .bind(schedule_id).bind(target_ids).bind(policy_id).bind(now).execute(pool).await.unwrap();
    }

    let job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: ResourceTarget::Target(target_id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: vec![],
        idempotency_key: format!("delete-target-job-{suffix}"),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    repositories.ansible_jobs.save(&job).await.unwrap();
    repositories
        .ansible_jobs
        .append_event(&JobEvent {
            sequence: 1,
            job_id: job.id,
            event: JobEventKind::Queued,
            created_at: now,
        })
        .await
        .unwrap();

    assert!(
        repositories
            .targets
            .delete_with_related_data(target_id, "admin")
            .await
            .unwrap()
    );
    assert!(
        repositories
            .targets
            .find_by_id(target_id)
            .await
            .unwrap()
            .is_none()
    );
    for (table, column) in [
        ("package_inventory_snapshots", "target_id"),
        ("target_telemetry_samples", "target_id"),
        ("target_docker_inventory", "target_id"),
        ("target_docker_telemetry_samples", "target_id"),
        ("ansible_jobs", "target_id"),
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = $1");
        let remaining: i64 = sqlx::query_scalar(&sql)
            .bind(target_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0, "rows remain in {table}");
    }
    let remaining_job: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ansible_jobs WHERE id = $1")
        .bind(job.id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(remaining_job, 0, "target workflow was not removed");
    let remaining_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ansible_job_events WHERE job_id = $1")
            .bind(job.id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        remaining_events, 0,
        "target workflow events were not removed"
    );
    let remaining_targets: Vec<Uuid> =
        sqlx::query_scalar("SELECT target_ids FROM schedules WHERE id = $1")
            .bind(&shared_schedule)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(remaining_targets, vec![other_target.as_uuid()]);
    let exclusive_schedule_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM schedules WHERE id = $1")
            .bind(&exclusive_schedule)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(exclusive_schedule_count, 0);
    let policies = repositories.update_policies.list().await.unwrap();
    let retained_policy = policies
        .iter()
        .find(|policy| policy.id == shared_policy.id)
        .unwrap();
    assert_eq!(retained_policy.allowed_targets, vec![other_target]);
    assert!(
        !policies
            .iter()
            .any(|policy| policy.id == exclusive_policy.id)
    );
    let tombstone_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE event_type = 'target.deleted' AND details->>'target_id' = $1")
        .bind(target_id.as_uuid().to_string()).fetch_one(pool).await.unwrap();
    assert_eq!(tombstone_count, 1);

    sqlx::query("DELETE FROM schedules WHERE id IN ($1, $2)")
        .bind(shared_schedule)
        .bind(exclusive_schedule)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM update_policies WHERE id IN ($1, $2)")
        .bind(shared_policy.id)
        .bind(exclusive_policy.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM audit_events WHERE event_type = 'target.deleted' AND details->>'target_id' = $1")
        .bind(target_id.as_uuid().to_string()).execute(pool).await.unwrap();
    test_database.finish().await;
}

#[tokio::test]
async fn deleting_target_rejects_active_workflow_without_removing_target() {
    let Some(test_database) = scoped_test_database("target_remove_busy").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let suffix = Uuid::new_v4().simple().to_string();
    let target = Target::new(
        format!("busy-delete-target-{suffix}"),
        TargetKind::Lxc,
        "192.0.2.91",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let mut job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: vec![],
        idempotency_key: format!("busy-delete-job-{suffix}"),
        confirmed: true,
        actor_role: ActorRole::Admin,
    })
    .unwrap();
    job.status = AnsibleJobStatus::Checking;
    repositories.ansible_jobs.save(&job).await.unwrap();

    assert!(matches!(
        repositories
            .targets
            .delete_with_related_data(target.id, "admin")
            .await,
        Err(lxcup_persistence::RepositoryError::TargetBusy)
    ));
    assert!(
        repositories
            .targets
            .find_by_id(target.id)
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query("DELETE FROM ansible_jobs WHERE id = $1")
        .bind(job.id.as_uuid())
        .execute(database.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM targets WHERE id = $1")
        .bind(target.id.as_uuid())
        .execute(database.pool())
        .await
        .unwrap();
    test_database.finish().await;
}
