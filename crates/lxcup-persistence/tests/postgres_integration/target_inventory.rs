use super::*;

#[tokio::test]
async fn postgres_repositories_cover_target_agent_and_inventory_crud() {
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
    let now = postgres_now();
    let suffix = Uuid::new_v4().simple().to_string();
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
    let telemetry_now = postgres_now();
    let heartbeat = AgentHeartbeat {
        target_id: target.id.as_uuid(),
        info: AgentInfo {
            agent_id: "integration-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "integration-host".to_owned(),
            version: "0.2.0".to_owned(),
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
                    collected_at: telemetry_now - chrono::Duration::seconds(31),
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
    };
    repositories
        .telemetry
        .append_heartbeat(&heartbeat)
        .await
        .unwrap();
    let samples = repositories.telemetry.list_recent(target.id).await.unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].cpu_basis_points, Some(1000));
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
        threshold: None,
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

    let inventory = PackageInventorySnapshot {
        target_id: target.id,
        collected_at: now,
        packages: vec![InstalledPackage {
            name: PackageName::new("curl").unwrap(),
            version: PackageVersion::new("8.5.0-2").unwrap(),
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
        .bind(container_id.value() as i64)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(node_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}
