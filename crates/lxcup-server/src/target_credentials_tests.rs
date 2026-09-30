use super::*;
use lxcup_core::{JobSchedule, ScheduleFrequency, SecretId, TargetKind, TargetTransport};
use std::time::Duration;

fn target(name: &str, secret_id: SecretId) -> Target {
    Target::new(
        name,
        TargetKind::LinuxServer,
        "192.0.2.241",
        TargetTransport::Ssh,
        SecretId::new(),
        secret_id,
    )
    .unwrap()
}

#[tokio::test]
async fn secret_revocation_resets_only_matching_enabled_agent_targets() {
    let secret = SecretId::new();
    let mut managed = target("managed", secret);
    managed.state = TargetState::Managed;
    let mut disabled = target("disabled", secret);
    disabled.state = TargetState::Disabled;
    let unrelated = target("unrelated", SecretId::new());
    let state = ApiState::new();
    state.store.write().await.targets = vec![managed.clone(), disabled.clone(), unrelated];

    let (affected, changed) = reset_targets_for_secret(&state, secret).await;
    assert_eq!(affected, vec![managed.id, disabled.id]);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].id, managed.id);
    assert_eq!(changed[0].state, TargetState::Pending);
    let targets = &state.store.read().await.targets;
    assert_eq!(targets[0].state, TargetState::Pending);
    assert_eq!(targets[1].state, TargetState::Disabled);
    let mut collected = Vec::new();
    reset_agent_target_if_needed(&mut disabled, secret, &mut collected);
    assert!(collected.is_empty());
}

#[tokio::test]
async fn persistence_helpers_are_noops_without_database_repositories() {
    let state = ApiState::new();
    invalidate_registered_agents(&state, SecretId::new()).await;
    persist_changed_targets(&state, vec![target("target", SecretId::new())]).await;
}

#[tokio::test]
async fn credential_invalidation_uses_repository_paths_without_matching_rows() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("credential_helpers_{}", uuid::Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        1,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let state = ApiState::new().with_repositories(repositories);

    invalidate_registered_agents(&state, SecretId::new()).await;
    persist_changed_targets(&state, vec![target("unpersisted", SecretId::new())]).await;
    let target_id = lxcup_core::TargetId::new();
    state.store.write().await.schedules.push(JobSchedule {
        id: "unpersisted-schedule".to_owned(),
        operation: "health_check".to_owned(),
        timezone: "UTC".to_owned(),
        target_ids: vec![target_id],
        frequency: ScheduleFrequency::EveryMinutes(30),
        enabled: true,
        threshold: None,
        policy_id: None,
        last_run_at: None,
        next_run_at: chrono::Utc::now(),
        last_error: None,
    });
    disable_schedules_for_targets(&state, vec![target_id]).await;
    assert!(!state.store.read().await.schedules[0].enabled);

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn revoking_a_secret_disables_only_enabled_schedules_that_reference_targets() {
    let affected = lxcup_core::TargetId::new();
    let other = lxcup_core::TargetId::new();
    let now = chrono::Utc::now();
    let schedule = |id: &str, target_ids: Vec<TargetId>, enabled| JobSchedule {
        id: id.to_owned(),
        operation: "health_check".to_owned(),
        timezone: "UTC".to_owned(),
        target_ids,
        frequency: ScheduleFrequency::EveryMinutes(30),
        enabled,
        threshold: None,
        policy_id: None,
        last_run_at: None,
        next_run_at: now,
        last_error: None,
    };
    let state = ApiState::new();
    state.store.write().await.schedules = vec![
        schedule("affected", vec![affected], true),
        schedule("unrelated", vec![other], true),
        schedule("already-disabled", vec![affected], false),
    ];

    disable_schedules_for_targets(&state, vec![affected]).await;

    let store = state.store.read().await;
    assert!(!store.schedules[0].enabled);
    assert_eq!(
        store.schedules[0].last_error.as_deref(),
        Some("disabled because a referenced secret was revoked or rotated")
    );
    assert!(store.schedules[1].enabled);
    assert!(store.schedules[1].last_error.is_none());
    assert!(!store.schedules[2].enabled);
    assert!(store.schedules[2].last_error.is_none());
}
