use super::*;
use lxcup_core::{
    JobSchedule, ScheduleFrequency, TargetTransport, ThresholdOperator, UpdatePolicy,
};

fn target(kind: TargetKind) -> Target {
    let mut target = Target::new(
        "scheduled-target",
        kind,
        "192.0.2.242",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    target
}

fn schedule(operation: &str, target_id: TargetId) -> JobSchedule {
    JobSchedule {
        id: "test-schedule".to_owned(),
        operation: operation.to_owned(),
        timezone: "UTC".to_owned(),
        target_ids: vec![target_id],
        frequency: ScheduleFrequency::EveryMinutes(30),
        enabled: true,
        threshold: None,
        policy_id: None,
        last_run_at: None,
        next_run_at: chrono::Utc::now(),
        last_error: None,
    }
}

#[tokio::test]
async fn scheduled_target_requires_managed_state_and_satisfied_threshold() {
    let state = ApiState::new();
    let managed = target(TargetKind::LinuxServer);
    let managed_id = managed.id;
    let mut pending = target(TargetKind::LinuxServer);
    pending.state = TargetState::Pending;
    let pending_id = pending.id;
    state.store.write().await.targets = vec![managed, pending];
    assert!(
        scheduled_target(&state, &schedule("health_check", managed_id), managed_id)
            .await
            .is_ok()
    );
    assert!(
        scheduled_target(
            &state,
            &schedule("health_check", TargetId::new()),
            TargetId::new()
        )
        .await
        .is_err()
    );
    assert!(
        scheduled_target(&state, &schedule("health_check", pending_id), pending_id)
            .await
            .is_err()
    );

    let mut threshold_schedule = schedule("health_check", managed_id);
    threshold_schedule.threshold = Some(ThresholdRule {
        metric: ThresholdMetric::CpuBasisPoints,
        operator: ThresholdOperator::GreaterThanOrEqual,
        value: 1,
    });
    let error = scheduled_target(&state, &threshold_schedule, managed_id)
        .await
        .unwrap_err();
    assert!(error.contains("threshold not met"));
}

#[tokio::test]
async fn scheduled_operations_map_registered_actions_and_validate_update_policy() {
    let target = target(TargetKind::LinuxServer);
    let target_id = target.id;
    let state = ApiState::new();
    let now = chrono::Utc::now();
    let health = scheduled_operation(
        &state,
        &schedule("health_check", target_id),
        target_id,
        &target,
        now,
    )
    .await
    .unwrap();
    assert_eq!(health.0, AnsibleOperation::HealthCheck);
    assert_eq!(health.2, ExecutionMode::Check);
    assert!(health.3.is_empty());

    let inventory = scheduled_operation(
        &state,
        &schedule("collect_package_inventory", target_id),
        target_id,
        &target,
        now,
    )
    .await
    .unwrap();
    assert_eq!(inventory.0, AnsibleOperation::CollectPackageInventory);
    assert_eq!(inventory.3, vec![target.credential_secret_ref]);
    assert!(
        scheduled_operation(
            &state,
            &schedule("unsupported", target_id),
            target_id,
            &target,
            now,
        )
        .await
        .unwrap_err()
        .contains("no longer registered")
    );

    let mut update = schedule("update_packages", target_id);
    assert!(
        scheduled_operation(&state, &update, target_id, &target, now)
            .await
            .unwrap_err()
            .contains("no valid policy")
    );
    let policy = UpdatePolicy {
        id: "nightly".to_owned(),
        allowed_targets: vec![target_id],
        allowed_packages: Vec::new(),
        maintenance_start_minute: 0,
        maintenance_end_minute: 1439,
        timezone: "UTC".to_owned(),
        maximum_risk: UpdateRisk::High,
        enabled: true,
    };
    update.policy_id = Some(policy.id.clone());
    state
        .store
        .write()
        .await
        .update_policies
        .push(policy.clone());
    assert!(
        scheduled_operation(&state, &update, target_id, &target, now)
            .await
            .unwrap_err()
            .contains("explicit package allowlist")
    );

    let mut restricted = policy.clone();
    restricted.allowed_packages = vec!["curl".to_owned()];
    restricted.enabled = false;
    state.store.write().await.update_policies[0] = restricted.clone();
    assert!(
        scheduled_operation(&state, &update, target_id, &target, now)
            .await
            .unwrap_err()
            .contains("outside its policy")
    );
    restricted.enabled = true;
    restricted.allowed_targets.clear();
    state.store.write().await.update_policies[0] = restricted.clone();
    assert!(
        scheduled_operation(&state, &update, target_id, &target, now)
            .await
            .unwrap_err()
            .contains("outside its policy")
    );
    restricted.allowed_targets.push(target_id);
    state.store.write().await.update_policies[0] = restricted;
    let package_update = scheduled_operation(&state, &update, target_id, &target, now)
        .await
        .unwrap();
    assert_eq!(package_update.0, AnsibleOperation::UpdatePackages);
    assert_eq!(package_update.2, ExecutionMode::Plan);
    assert_eq!(package_update.3, vec![target.credential_secret_ref]);
    assert_eq!(
        package_update.1,
        AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()]
        }
    );
}

#[tokio::test]
async fn docker_discovery_schedule_requires_an_lxc_with_a_connected_host() {
    let state = ApiState::new();
    let linux = target(TargetKind::LinuxServer);
    let linux_id = linux.id;
    let lxc = target(TargetKind::Lxc);
    let lxc_id = lxc.id;
    state.store.write().await.targets = vec![linux, lxc];
    let now = chrono::Utc::now();
    assert!(
        dispatch_scheduled_target(
            &state,
            &schedule("docker_discovery", linux_id),
            linux_id,
            now,
        )
        .await
        .unwrap_err()
        .contains("require an LXC")
    );
    assert!(
        dispatch_scheduled_target(&state, &schedule("docker_discovery", lxc_id), lxc_id, now,)
            .await
            .unwrap_err()
            .contains("no connected Docker discovery host")
    );
}
