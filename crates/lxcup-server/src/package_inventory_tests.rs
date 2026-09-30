use super::*;
use chrono::{Timelike, Utc};
use lxcup_core::{
    InstalledPackage, PackageInventorySnapshot, PackageName, PackageVersion, SecretId, Target,
    TargetKind, TargetTransport,
};
use std::time::Duration;

fn target() -> Target {
    Target::new(
        "package-inventory-target",
        TargetKind::LinuxServer,
        "192.0.2.240",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn rejects_invalid_and_unknown_target_ids() {
    let state = ApiState::new();
    let invalid = get_package_inventory(State(state.clone()), Path("invalid".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(invalid.code, "invalid_id");

    let missing = get_package_inventory(State(state), Path(uuid::Uuid::new_v4().to_string()))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "not_found");
}

#[tokio::test]
async fn returns_complete_persisted_inventory_with_optional_metadata() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("package_api_{}", uuid::Uuid::new_v4().simple());
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
    let target = target();
    let target_id = target.id;
    let state = ApiState::new().with_repositories(repositories.clone());
    state.store.write().await.targets.push(target);
    repositories
        .targets
        .save(&state.store.read().await.targets[0])
        .await
        .unwrap();
    let collected_at = Utc::now().with_nanosecond(0).unwrap();
    repositories
        .package_inventory
        .replace(&PackageInventorySnapshot {
            target_id,
            collected_at,
            packages: vec![InstalledPackage {
                name: PackageName::new("curl").unwrap(),
                version: PackageVersion::new("8.5.0-2").unwrap(),
                candidate_version: Some(PackageVersion::new("8.6.0-1").unwrap()),
                architecture: Some("amd64".to_owned()),
                source: Some("apt".to_owned()),
            }],
        })
        .await
        .unwrap();

    let response = get_package_inventory(State(state), Path(target_id.as_uuid().to_string()))
        .await
        .unwrap()
        .0;
    assert_eq!(response.data.status, "complete");
    assert_eq!(response.data.collected_at, Some(collected_at));
    assert_eq!(response.data.packages.len(), 1);
    assert_eq!(response.data.packages[0].name, "curl");
    assert_eq!(response.data.packages[0].installed_version, "8.5.0-2");
    assert_eq!(
        response.data.packages[0].candidate_version.as_deref(),
        Some("8.6.0-1")
    );
    assert_eq!(
        response.data.packages[0].architecture.as_deref(),
        Some("amd64")
    );
    assert_eq!(response.data.packages[0].source.as_deref(), Some("apt"));

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}
