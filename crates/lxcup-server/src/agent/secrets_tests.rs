use super::*;
use std::time::Duration;

#[tokio::test]
async fn secret_audit_reads_redacted_events_from_persistence_and_normalizes_roles() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("secret_audit_{}", uuid::Uuid::new_v4().simple());
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
    let state = ApiState::new().with_repositories(repositories.clone());
    let admin_secret = SecretId::new();
    record_secret_audit(&state, admin_secret, "created", ActorRole::Admin, None)
        .await
        .unwrap();
    for (role, action) in [("operator", "rotated"), ("unexpected", "revoked")] {
        repositories
            .audit_events
            .append(&lxcup_persistence::AuditEvent {
                id: uuid::Uuid::new_v4(),
                node_id: None,
                container_id: None,
                plan_id: None,
                execution_id: None,
                event_type: format!("secret.{action}"),
                details: serde_json::json!({
                    "secret_id": SecretId::new().as_uuid().to_string(),
                    "role": role
                }),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
    }
    repositories
        .audit_events
        .append(&lxcup_persistence::AuditEvent {
            id: uuid::Uuid::new_v4(),
            node_id: None,
            container_id: None,
            plan_id: None,
            execution_id: None,
            event_type: "secret.malformed".to_owned(),
            details: serde_json::json!({"secret_id": "not-a-uuid", "role": "admin"}),
            created_at: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let response = list_secret_audit(State(state)).await.unwrap().0;
    assert_eq!(response.data.len(), 3);
    assert!(response.data.iter().any(|event| {
        event.secret_id == admin_secret
            && event.action == "created"
            && event.role == ActorRole::Admin
    }));
    assert!(
        response
            .data
            .iter()
            .any(|event| event.action == "rotated" && event.role == ActorRole::Operator)
    );
    assert!(
        response
            .data
            .iter()
            .any(|event| event.action == "revoked" && event.role == ActorRole::Viewer)
    );

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}
