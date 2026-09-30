use super::*;

#[tokio::test]
async fn postgres_repositories_cover_target_agent_and_inventory_crud() {
    let Some(test_database) = scoped_test_database("target_inventory").await else {
        return;
    };
    let database = test_database.database();
    let repositories = Repositories::new(database);
    let suffix = Uuid::new_v4().simple().to_string();
    let persisted_policy = UpdatePolicy {
        id: "postgres-integration-package-policy".to_owned(),
        allowed_targets: vec![lxcup_core::TargetId::new()],
        allowed_packages: vec!["curl".to_owned()],
        maintenance_start_minute: 60,
        maintenance_end_minute: 120,
        timezone: "UTC".to_owned(),
        maximum_risk: UpdateRisk::High,
        enabled: true,
    };
    repositories
        .update_policies
        .save(&persisted_policy)
        .await
        .unwrap();
    assert!(
        repositories
            .update_policies
            .list()
            .await
            .unwrap()
            .contains(&persisted_policy)
    );
    assert!(
        repositories
            .update_policies
            .delete(&persisted_policy.id)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .update_policies
            .delete(&persisted_policy.id)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .update_policies
            .list()
            .await
            .unwrap()
            .contains(&persisted_policy)
    );
    let mut default_policy = persisted_policy.clone();
    default_policy.id = format!("generated-{suffix}");
    assert!(
        repositories
            .update_policies
            .save_if_absent(&default_policy)
            .await
            .unwrap()
    );
    let mut edited_default = default_policy.clone();
    edited_default.enabled = false;
    repositories
        .update_policies
        .save(&edited_default)
        .await
        .unwrap();
    assert!(
        !repositories
            .update_policies
            .save_if_absent(&default_policy)
            .await
            .unwrap()
    );
    assert!(
        repositories
            .update_policies
            .list()
            .await
            .unwrap()
            .contains(&edited_default)
    );
    assert!(
        repositories
            .update_policies
            .delete(&default_policy.id)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .update_policies
            .delete(&default_policy.id)
            .await
            .unwrap()
    );
    let now = postgres_now();
    let container_id = ContainerId::new((Uuid::new_v4().as_u128() as u64 % 900_000_000) + 10_000);
    let node = Node::new(
        format!("repository-crud-node-{suffix}"),
        "https://pve.example.test",
    )
    .unwrap();
    let node_id = node.id;
    repositories.nodes.save(&node).await.unwrap();
    let mut container = Container::new(
        container_id,
        node_id,
        format!("repository-crud-{suffix}"),
        OperatingSystem::Debian,
        ContainerStatus::Running,
    )
    .unwrap();
    assert!(
        repositories
            .containers
            .upsert_discovered(&container)
            .await
            .unwrap()
    );
    assert!(
        !repositories
            .containers
            .upsert_discovered(&container)
            .await
            .unwrap()
    );
    container
        .transition_management_to(lxcup_core::ContainerManagementState::Managed)
        .unwrap();
    repositories
        .containers
        .update_management_state(&container)
        .await
        .unwrap();
    repositories
        .containers
        .mark_status_unknown(container_id)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .containers
            .list_by_node(node_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let additional_container_id = ContainerId::new(container_id.value() + 1);
    let additional_container = Container::new(
        additional_container_id,
        node_id,
        format!("repository-save-{suffix}"),
        OperatingSystem::Ubuntu,
        ContainerStatus::Stopped,
    )
    .unwrap();
    repositories
        .containers
        .save(&additional_container)
        .await
        .unwrap();
    let saved_additional_container = repositories
        .containers
        .find_by_id(additional_container_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved_additional_container.name, additional_container.name);
    assert_eq!(
        saved_additional_container.operating_system,
        additional_container.operating_system
    );

    let workload = DockerWorkload {
        host_container_id: container_id,
        id: format!("docker-crud-{suffix}"),
        name: "web".to_owned(),
        image: "nginx:latest".to_owned(),
        state: "running".to_owned(),
        status: "Up".to_owned(),
        ports: vec!["80/tcp".to_owned()],
        started_at: Some("2026-01-01T00:00:00Z".to_owned()),
        labels: vec!["app=web".to_owned()],
        presence: lxcup_core::DockerWorkloadPresence::Present,
        change: lxcup_core::DockerWorkloadChange::New,
        management_state: DockerWorkloadManagementState::Discovered,
        discovered_at: now,
    };
    assert_eq!(
        repositories
            .docker_workloads
            .upsert_discovered(&workload)
            .await
            .unwrap(),
        lxcup_core::DockerWorkloadChange::New
    );
    assert_eq!(
        repositories
            .docker_workloads
            .upsert_discovered(&workload)
            .await
            .unwrap(),
        lxcup_core::DockerWorkloadChange::Unchanged
    );
    let mut changed_workload = workload.clone();
    changed_workload.image = "nginx:stable".to_owned();
    assert_eq!(
        repositories
            .docker_workloads
            .upsert_discovered(&changed_workload)
            .await
            .unwrap(),
        lxcup_core::DockerWorkloadChange::Changed
    );
    repositories
        .docker_workloads
        .mark_missing_except(container_id, &[])
        .await
        .unwrap();
    assert_eq!(
        repositories
            .docker_workloads
            .list(container_id)
            .await
            .unwrap()[0]
            .change,
        lxcup_core::DockerWorkloadChange::Missing
    );
    let discovery = repositories
        .docker_discovery
        .start(container_id)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .docker_discovery
            .latest(container_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        lxcup_core::DockerDiscoveryStatus::Running
    );
    let completed_discovery = repositories
        .docker_discovery
        .finish(
            discovery.id,
            lxcup_core::DockerDiscoveryStatus::Succeeded,
            1,
            None,
        )
        .await
        .unwrap();
    assert_eq!(completed_discovery.container_count, 1);
    assert!(
        repositories
            .docker_workloads
            .adopt(container_id, &workload.id)
            .await
            .unwrap()
    );
    assert_eq!(
        repositories
            .docker_workloads
            .list(container_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repositories
            .docker_workloads
            .remove(container_id, &workload.id)
            .await
            .unwrap()
    );

    let registration = AgentRegistration::new(
        container_id,
        format!("agent-crud-{suffix}"),
        "http://127.0.0.1:8090",
        SecretId::new(),
        None,
        now,
    )
    .unwrap();
    repositories
        .agent_registrations
        .save(&registration)
        .await
        .unwrap();
    assert!(
        repositories
            .agent_registrations
            .find_by_container(container_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repositories
            .agent_registrations
            .list()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == registration.id)
    );

    let target = Target::new(
        format!("target-crud-{suffix}"),
        TargetKind::LinuxServer,
        "192.0.2.10",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repositories.targets.save(&target).await.unwrap();
    let mut docker_inventory = lxcup_persistence::PersistedTargetDockerInventory {
        collected_at: now,
        containers: vec![lxcup_agent::DockerContainerInfo {
            id: "docker-target-integration".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: None,
            created_at: None,
            image_id: None,
            restart_count: None,
            health: None,
            oom_killed: None,
            labels: vec!["app=web".to_owned()],
        }],
        events: Vec::new(),
    };
    repositories
        .target_docker_inventory
        .save(target.id, &mut docker_inventory)
        .await
        .unwrap();
    assert_eq!(
        repositories
            .target_docker_inventory
            .get(target.id)
            .await
            .unwrap()
            .unwrap()
            .containers,
        docker_inventory.containers
    );
    assert!(
        repositories
            .target_docker_inventory
            .get(target.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .is_empty()
    );
    docker_inventory.collected_at = now + chrono::Duration::seconds(30);
    docker_inventory.containers[0].image = "nginx:2".to_owned();
    repositories
        .target_docker_inventory
        .save(target.id, &mut docker_inventory)
        .await
        .unwrap();
    assert!(
        repositories
            .target_docker_inventory
            .get(target.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .iter()
            .any(|event| event.kind == lxcup_persistence::DockerInventoryEventKind::ImageChanged)
    );
    let telemetry_now = postgres_now();
    let heartbeat = AgentHeartbeat {
        target_id: target.id.as_uuid(),
        info: AgentInfo {
            agent_id: "integration-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "integration-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: "v1".to_owned(),
        },
        metrics: AgentMetrics {
            collected_at: telemetry_now,
            commands_total: 0,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: telemetry_now,
        telemetry: SystemTelemetryWindow {
            samples: vec![
                SystemTelemetrySample {
                    collected_at: telemetry_now,
                    cpu_basis_points: Some(1000),
                    memory_basis_points: Some(2000),
                    storage_basis_points: Some(3000),
                    load_1_milli: Some(100),
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: Some(4),
                },
                SystemTelemetrySample {
                    collected_at: telemetry_now - chrono::Duration::seconds(61),
                    cpu_basis_points: Some(9000),
                    memory_basis_points: Some(9000),
                    storage_basis_points: Some(9000),
                    load_1_milli: Some(900),
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: Some(9),
                },
            ],
            partial: false,
        },
        docker_telemetry: lxcup_agent::DockerTelemetryWindow {
            samples: vec![
                lxcup_agent::DockerTelemetrySample {
                    collected_at: telemetry_now,
                    container_id: "ABCDEF012345".to_owned(),
                    cpu_basis_points: Some(500),
                    memory_basis_points: Some(2_500),
                    memory_used_bytes: Some(25),
                    memory_limit_bytes: Some(100),
                },
                lxcup_agent::DockerTelemetrySample {
                    collected_at: telemetry_now,
                    container_id: "not-hex".to_owned(),
                    cpu_basis_points: None,
                    memory_basis_points: None,
                    memory_used_bytes: None,
                    memory_limit_bytes: None,
                },
                lxcup_agent::DockerTelemetrySample {
                    collected_at: telemetry_now,
                    container_id: "0123456789ab".to_owned(),
                    cpu_basis_points: None,
                    memory_basis_points: Some(10_001),
                    memory_used_bytes: None,
                    memory_limit_bytes: None,
                },
            ],
        },
    };
    let historical_sample = SystemTelemetrySample {
        collected_at: telemetry_now - chrono::Duration::minutes(9),
        cpu_basis_points: Some(9000),
        memory_basis_points: Some(9000),
        storage_basis_points: Some(9000),
        load_1_milli: Some(900),
        network_rx_bytes: None,
        network_tx_bytes: None,
        process_count: Some(9),
    };
    let expired_sample = SystemTelemetrySample {
        collected_at: telemetry_now - chrono::Duration::minutes(11),
        ..historical_sample.clone()
    };
    for sample in [&historical_sample, &expired_sample] {
        sqlx::query("INSERT INTO target_telemetry_samples (target_id, collected_at, payload) VALUES ($1, $2, $3)")
            .bind(target.id.as_uuid())
            .bind(sample.collected_at)
            .bind(serde_json::to_value(sample).unwrap())
            .execute(database.pool())
            .await
            .unwrap();
    }
    repositories
        .telemetry
        .append_heartbeat(&heartbeat)
        .await
        .unwrap();
    let mut overlapping_heartbeat = heartbeat.clone();
    overlapping_heartbeat.telemetry.samples = vec![SystemTelemetrySample {
        collected_at: telemetry_now + chrono::Duration::milliseconds(500),
        cpu_basis_points: Some(5000),
        memory_basis_points: Some(6000),
        storage_basis_points: Some(7000),
        load_1_milli: Some(500),
        network_rx_bytes: None,
        network_tx_bytes: None,
        process_count: Some(6),
    }];
    repositories
        .telemetry
        .append_heartbeat(&overlapping_heartbeat)
        .await
        .unwrap();
    let samples = repositories.telemetry.list_recent(target.id).await.unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].cpu_basis_points, Some(9000));
    assert_eq!(samples[1].cpu_basis_points, Some(1000));
    let docker_samples = repositories
        .telemetry
        .list_docker_recent(target.id)
        .await
        .unwrap();
    assert_eq!(docker_samples.len(), 1);
    assert_eq!(docker_samples[0].container_id, "ABCDEF012345");

    let expired_at = postgres_now() - chrono::Duration::days(31);
    let expired_sample = SystemTelemetrySample {
        collected_at: expired_at,
        ..historical_sample.clone()
    };
    sqlx::query("INSERT INTO target_telemetry_samples (target_id, collected_at, payload) VALUES ($1, $2, $3)")
        .bind(target.id.as_uuid())
        .bind(expired_sample.collected_at)
        .bind(serde_json::to_value(expired_sample).unwrap())
        .execute(database.pool())
        .await
        .unwrap();
    let expired_docker_sample = lxcup_agent::DockerTelemetrySample {
        collected_at: expired_at,
        container_id: "0123456789ab".to_owned(),
        cpu_basis_points: Some(100),
        memory_basis_points: Some(200),
        memory_used_bytes: None,
        memory_limit_bytes: None,
    };
    sqlx::query("INSERT INTO target_docker_telemetry_samples (target_id, container_id, collected_at, payload) VALUES ($1, $2, $3, $4)")
        .bind(target.id.as_uuid())
        .bind(&expired_docker_sample.container_id)
        .bind(expired_docker_sample.collected_at)
        .bind(serde_json::to_value(&expired_docker_sample).unwrap())
        .execute(database.pool())
        .await
        .unwrap();
    assert!(repositories.telemetry.prune_expired().await.unwrap() >= 2);
    let expired_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM target_telemetry_samples WHERE target_id=$1 AND collected_at=$2",
    )
    .bind(target.id.as_uuid())
    .bind(expired_at)
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(expired_rows, 0);
    let expired_docker_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM target_docker_telemetry_samples WHERE target_id=$1 AND collected_at=$2",
    )
    .bind(target.id.as_uuid())
    .bind(expired_at)
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(expired_docker_rows, 0);

    let docker_inventory_row =
        sqlx::query("SELECT containers FROM target_docker_inventory WHERE target_id=$1")
            .bind(target.id.as_uuid())
            .fetch_one(database.pool())
            .await
            .unwrap();
    let containers: serde_json::Value = sqlx::Row::get(&docker_inventory_row, "containers");
    let docker_container_id = containers[0]["id"].as_str().unwrap();
    let old_event = json!({
        "observed_at": (postgres_now() - chrono::Duration::days(181)),
        "container_id": docker_container_id,
        "container_name": "web",
        "kind": "added",
        "previous_value": null,
        "current_value": "present"
    });
    let recent_event = json!({
        "observed_at": postgres_now(),
        "container_id": docker_container_id,
        "container_name": "web",
        "kind": "state_changed",
        "previous_value": "exited",
        "current_value": "running"
    });
    sqlx::query("UPDATE target_docker_inventory SET events=$2 WHERE target_id=$1")
        .bind(target.id.as_uuid())
        .bind(json!([old_event, recent_event]))
        .execute(database.pool())
        .await
        .unwrap();
    assert_eq!(
        repositories
            .target_docker_inventory
            .prune_expired_events()
            .await
            .unwrap(),
        1
    );
    let retained_events = repositories
        .target_docker_inventory
        .get(target.id)
        .await
        .unwrap()
        .unwrap()
        .events;
    assert_eq!(retained_events.len(), 1);
    assert_eq!(
        retained_events[0].kind,
        lxcup_persistence::DockerInventoryEventKind::StateChanged
    );
    assert!(
        repositories
            .targets
            .find_by_id(target.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repositories
            .targets
            .list()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == target.id)
    );
    let mut managed = target.clone();
    managed.mark_managed();
    repositories.targets.update(&managed).await.unwrap();
    assert_eq!(
        repositories
            .targets
            .find_by_id(target.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        lxcup_core::TargetState::Managed
    );

    let schedule = JobSchedule {
        id: format!("inventory-{suffix}"),
        operation: "collect_package_inventory".to_owned(),
        timezone: "Europe/Berlin".to_owned(),
        target_ids: vec![target.id],
        frequency: ScheduleFrequency::EveryMinutes(60),
        enabled: true,
        threshold: Some(lxcup_core::ThresholdRule {
            metric: lxcup_core::ThresholdMetric::CpuBasisPoints,
            operator: lxcup_core::ThresholdOperator::GreaterThanOrEqual,
            value: 8_000,
        }),
        policy_id: None,
        last_run_at: None,
        next_run_at: now + chrono::Duration::hours(1),
        last_error: None,
    };
    repositories.schedules.save(&schedule).await.unwrap();
    assert!(
        repositories
            .schedules
            .list()
            .await
            .unwrap()
            .iter()
            .any(|persisted| persisted == &schedule)
    );
    let mut updated_schedule = schedule.clone();
    updated_schedule.enabled = false;
    updated_schedule.last_error = Some("paused for integration coverage".to_owned());
    repositories
        .schedules
        .update(&updated_schedule)
        .await
        .unwrap();
    assert!(
        repositories
            .schedules
            .list()
            .await
            .unwrap()
            .iter()
            .any(|persisted| persisted == &updated_schedule)
    );
    let invalid_schedule = JobSchedule {
        id: format!("invalid-{suffix}"),
        frequency: ScheduleFrequency::EveryMinutes(u32::MAX),
        ..schedule.clone()
    };
    assert!(matches!(
        repositories.schedules.save(&invalid_schedule).await,
        Err(lxcup_persistence::RepositoryError::InvalidValue {
            field: "schedule interval"
        })
    ));

    let inventory = PackageInventorySnapshot {
        target_id: target.id,
        collected_at: now,
        packages: vec![InstalledPackage {
            name: PackageName::new("curl").unwrap(),
            version: PackageVersion::new("8.5.0-2").unwrap(),
            candidate_version: Some(PackageVersion::new("8.6.0-1").unwrap()),
            architecture: Some("amd64".to_owned()),
            source: Some("apt".to_owned()),
        }],
    };
    repositories
        .package_inventory
        .replace(&inventory)
        .await
        .unwrap();
    let loaded_inventory = repositories
        .package_inventory
        .find_latest(target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_inventory.snapshot, inventory);
    let empty_inventory = PackageInventorySnapshot {
        packages: Vec::new(),
        ..inventory
    };
    repositories
        .package_inventory
        .replace(&empty_inventory)
        .await
        .unwrap();
    assert!(
        repositories
            .package_inventory
            .find_latest(target.id)
            .await
            .unwrap()
            .unwrap()
            .snapshot
            .packages
            .is_empty()
    );

    let pool = database.pool();
    sqlx::query("DELETE FROM targets WHERE id = $1")
        .bind(target.id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_registrations WHERE container_id = $1")
        .bind(container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM containers WHERE id = $1")
        .bind(additional_container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM containers WHERE id = $1")
        .bind(container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(node_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    test_database.finish().await;
}
