use super::*;

#[tokio::test]
async fn auth_repository_persists_sessions_audit_and_guards_the_last_admin() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("auth_users_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = DatabaseConfig::from_values(
        scoped_url,
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
    let suffix = Uuid::new_v4().simple().to_string();
    let admin_name = format!("test-admin-{suffix}");
    let second_admin_name = format!("test-admin-secondary-{suffix}");
    let password_hash = format!("test-password-hash-{suffix}");
    let admin = repositories
        .auth
        .create_user(
            &admin_name,
            &password_hash,
            lxcup_persistence::AuthUserRole::Admin,
            true,
        )
        .await
        .unwrap();
    let second_admin = repositories
        .auth
        .create_user(
            &second_admin_name,
            &password_hash,
            lxcup_persistence::AuthUserRole::Admin,
            false,
        )
        .await
        .unwrap();
    let token_hash = format!("session-token-hash-{suffix}");
    let expires_at = Utc::now() + chrono::Duration::hours(1);
    repositories
        .auth
        .save_session(admin.id, &token_hash, expires_at)
        .await
        .unwrap();
    let other_token_hash = format!("other-session-token-hash-{suffix}");
    repositories
        .auth
        .save_session(admin.id, &other_token_hash, expires_at)
        .await
        .unwrap();
    let session = repositories
        .auth
        .session_by_token_hash(&token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.user.username, admin_name);
    assert_eq!(session.user.role, lxcup_persistence::AuthUserRole::Admin);

    repositories
        .auth
        .change_password(admin.id, "changed-hash", &token_hash)
        .await
        .unwrap();
    assert!(
        repositories
            .auth
            .session_by_token_hash(&token_hash)
            .await
            .unwrap()
            .is_some()
    );
    let updated = repositories
        .auth
        .user_by_id(admin.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.password_hash, "changed-hash");
    assert!(!updated.must_change_password);
    assert!(
        repositories
            .auth
            .session_by_token_hash(&other_token_hash)
            .await
            .unwrap()
            .is_none()
    );

    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::User, false)
        .await
        .unwrap();
    assert!(
        repositories
            .auth
            .session_by_token_hash(&token_hash)
            .await
            .unwrap()
            .is_none(),
        "role changes revoke every session for the affected account"
    );
    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::Admin, false)
        .await
        .unwrap();
    let reset_session_hash = format!("reset-session-token-hash-{suffix}");
    repositories
        .auth
        .save_session(admin.id, &reset_session_hash, expires_at)
        .await
        .unwrap();
    repositories
        .auth
        .reset_user_password(admin.id, "reset-password-hash")
        .await
        .unwrap();
    assert!(
        repositories
            .auth
            .session_by_token_hash(&reset_session_hash)
            .await
            .unwrap()
            .is_none(),
        "password resets revoke every session for the affected account"
    );
    assert!(
        repositories
            .auth
            .user_by_id(admin.id)
            .await
            .unwrap()
            .unwrap()
            .must_change_password
    );

    let disabled_session_hash = format!("disabled-session-token-hash-{suffix}");
    repositories
        .auth
        .save_session(admin.id, &disabled_session_hash, expires_at)
        .await
        .unwrap();
    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::Admin, true)
        .await
        .unwrap();
    assert!(
        repositories
            .auth
            .session_by_token_hash(&disabled_session_hash)
            .await
            .unwrap()
            .is_none(),
        "disabling an account revokes every session for that account"
    );
    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::Admin, false)
        .await
        .unwrap();

    let event = lxcup_persistence::AuthAuditEvent {
        id: Uuid::new_v4(),
        actor_user_id: Some(admin.id),
        actor_username: admin_name.clone(),
        actor_role: lxcup_persistence::AuthUserRole::Admin,
        action: format!("test.action.{suffix}"),
        resource_type: "test".to_owned(),
        resource_id: Some(suffix.clone()),
        request_id: Some(Uuid::new_v4()),
        status_code: 102,
        details: json!({"safe": true}),
        created_at: Utc::now(),
    };
    repositories.auth.append_audit_event(&event).await.unwrap();
    assert!(
        repositories
            .auth
            .update_audit_event_status(event.id, 200, "completed")
            .await
            .unwrap()
    );
    let listed = repositories
        .auth
        .list_audit_events(&lxcup_persistence::AuthAuditFilters {
            limit: 1,
            actor_username: Some(admin_name.clone()),
            action: Some("test.action".to_owned()),
            resource: Some(suffix.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        listed
            .events
            .iter()
            .any(|candidate| candidate.id == event.id)
    );
    assert_eq!(listed.total, 1);
    assert_eq!(listed.events[0].status_code, 200);
    assert_eq!(listed.events[0].details["state"], "completed");
    assert_eq!(listed.limit, 1);
    assert_eq!(listed.offset, 0);

    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::User, false)
        .await
        .unwrap();
    assert!(matches!(
        repositories.auth.delete_user(second_admin.id).await,
        Err(lxcup_persistence::RepositoryError::LastAdmin)
    ));
    repositories
        .auth
        .update_user(admin.id, lxcup_persistence::AuthUserRole::Admin, false)
        .await
        .unwrap();
    repositories
        .auth
        .delete_user(second_admin.id)
        .await
        .unwrap();
    assert!(matches!(
        repositories
            .auth
            .update_user(admin.id, lxcup_persistence::AuthUserRole::User, false)
            .await,
        Err(lxcup_persistence::RepositoryError::LastAdmin)
    ));
    assert!(matches!(
        repositories
            .auth
            .update_user(admin.id, lxcup_persistence::AuthUserRole::Admin, true)
            .await,
        Err(lxcup_persistence::RepositoryError::LastAdmin)
    ));
    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}
