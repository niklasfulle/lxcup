use super::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn bootstrap_admin_is_atomic_one_time_and_forces_the_first_password_change() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("auth_bootstrap_{}", uuid::Uuid::new_v4().simple());
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
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT current_schema()")
            .fetch_one(database.pool())
            .await
            .unwrap(),
        schema
    );

    let repositories = lxcup_persistence::Repositories::new(&database);
    let state = ApiState::new()
        .with_repositories(repositories.clone())
        .with_account_auth();
    let (first, second) = tokio::join!(
        state.initialize_bootstrap_admin(),
        state.initialize_bootstrap_admin()
    );
    assert_ne!(first.unwrap(), second.unwrap());

    let api = router(state.clone());
    let (status, first_login_headers, first_login) = post_json_with_headers(
        api.clone(),
        "/api/v1/auth/login",
        json!({"username": "admin", "password": "admin"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first_login["data"]["user"]["must_change_password"], true);
    assert!(first_login["data"].get("access_token").is_none());
    let token = session_cookie(&first_login_headers);
    let csrf = csrf_cookie(&first_login_headers);
    let (status, session) = get_json_with_cookie(api.clone(), "/api/v1/auth/session", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["data"]["must_change_password"], true);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/auth/password",
        json!({"new_password": format!("Admin-first-login-{schema}")}),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get_json(api.clone(), "/api/v1/targets", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let new_password = format!("Admin-first-login-{schema}");
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/auth/password",
        json!({"new_password": new_password}),
        &token,
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!state.initialize_bootstrap_admin().await.unwrap());

    let (status, _) = post_json(
        api.clone(),
        "/api/v1/auth/login",
        json!({"username": "admin", "password": "admin"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, later_login) = post_json(
        api,
        "/api/v1/auth/login",
        json!({"username": "admin", "password": new_password}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(later_login["data"]["user"]["must_change_password"], false);

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn account_auth_enforces_first_login_admin_routes_logout_and_redacted_audit() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("auth_account_{}", uuid::Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        3,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT current_schema()")
            .fetch_one(database.pool())
            .await
            .unwrap(),
        schema
    );
    let repositories = lxcup_persistence::Repositories::new(&database);
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let admin_name = format!("api-admin-{suffix}");
    let user_name = format!("api-user-{suffix}");
    let admin_password = format!("Admin-password-{suffix}");
    let temporary_password = format!("Temporary-password-{suffix}");
    let admin_hash = crate::user_auth::hash_password(&admin_password).unwrap();
    let user_hash = crate::user_auth::hash_password(&temporary_password).unwrap();
    let admin = repositories
        .auth
        .create_user(
            &admin_name,
            &admin_hash,
            lxcup_persistence::AuthUserRole::Admin,
            false,
        )
        .await
        .unwrap();
    let managed_username = format!("api-created-user-{suffix}");
    let user = repositories
        .auth
        .create_user(
            &user_name,
            &user_hash,
            lxcup_persistence::AuthUserRole::User,
            true,
        )
        .await
        .unwrap();
    let api = router(
        ApiState::new()
            .with_repositories(repositories.clone())
            .with_account_auth(),
    );

    let (status, admin_login_headers, admin_login) = post_json_with_headers(
        api.clone(),
        "/api/v1/auth/login",
        json!({"username": admin_name, "password": admin_password}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(admin_login["data"].get("access_token").is_none());
    let admin_token = session_cookie(&admin_login_headers);
    let admin_csrf = csrf_cookie(&admin_login_headers);
    let (status, _) = get_json_with_cookie(api.clone(), "/api/v1/auth/session", &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    let (status, created_user) = post_json_with_cookie(
        api.clone(),
        "/api/v1/users",
        json!({
            "username": managed_username,
            "role": "user",
            "password": format!("Temporary-created-{suffix}")
        }),
        &admin_token,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/users",
        json!({"username": "invalid username", "role": "user", "password": "Temporary-created-password"}),
        &admin_token,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/users",
        json!({"username": format!("  {managed_username}  "), "role": "user", "password": "Temporary-created-password"}),
        &admin_token,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let managed_id = created_user["data"]["id"].as_str().unwrap();
    let (status, _) = patch_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}"),
        json!({"role": "viewer", "disabled": false}),
        &admin_token,
        &admin_csrf,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, updated) = patch_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}"),
        json!({"role": "admin", "disabled": false}),
        &admin_token,
        &admin_csrf,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["role"], "admin");
    let reset_password = format!("Admin-reset-password-{suffix}");
    let (status, _) = post_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}/password-reset"),
        json!({"password": reset_password}),
        &admin_token,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}/password-reset"),
        json!({"password": "short"}),
        &admin_token,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, updated) = patch_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}"),
        json!({"role": "user", "disabled": true}),
        &admin_token,
        &admin_csrf,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["disabled"], true);
    let (status, _) = delete_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}"),
        json!({"confirmed": false}),
        &admin_token,
        &admin_csrf,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = delete_json_with_cookie(
        api.clone(),
        &format!("/api/v1/users/{managed_id}"),
        json!({"confirmed": true}),
        &admin_token,
        &admin_csrf,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (failed_status, _) = post_json(
        api.clone(),
        "/api/v1/auth/login",
        json!({"username": admin_name, "password": "wrong-password"}),
        None,
    )
    .await;
    assert_eq!(failed_status, StatusCode::UNAUTHORIZED);

    let (status, users_response) = get_json(api.clone(), "/api/v1/users", &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    let serialized = users_response.to_string();
    assert!(serialized.contains(&user_name));
    assert!(!serialized.contains("password_hash"));
    let (status, audit_response) =
        get_json(api.clone(), "/api/v1/auth/audit?limit=10", &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        audit_response["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "auth.login_failed")
    );
    let (status, filtered_audit) = get_json(
        api.clone(),
        &format!("/api/v1/auth/audit?limit=10&offset=0&actor_username={admin_name}&action=user.created&resource=user"),
        &admin_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        filtered_audit["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "user.created")
    );
    assert!(!audit_response.to_string().contains("wrong-password"));

    let (status, user_login_headers, user_login) = post_json_with_headers(
        api.clone(),
        "/api/v1/auth/login",
        json!({"username": user_name, "password": temporary_password}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user_login["data"]["user"]["must_change_password"], true);
    assert!(user_login["data"].get("access_token").is_none());
    let user_token = session_cookie(&user_login_headers);
    let user_csrf = csrf_cookie(&user_login_headers);
    let (status, _) = get_json(api.clone(), "/api/v1/users", &user_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get_json(api.clone(), "/api/v1/auth/audit", &user_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get_json(api.clone(), "/api/v1/targets", &user_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/auth/password",
        json!({"new_password": format!("Rotated-password-{suffix}")}),
        &user_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post_json_with_cookie(
        api.clone(),
        "/api/v1/auth/password",
        json!({"new_password": format!("Rotated-password-{suffix}")}),
        &user_token,
        Some(&user_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, session) = get_json(api.clone(), "/api/v1/auth/session", &user_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["data"]["must_change_password"], false);
    let (status, _) = get_json(api.clone(), "/api/v1/targets", &user_token).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get_json(api.clone(), "/api/v1/users", &user_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get_json(api.clone(), "/api/v1/auth/audit", &user_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post_json(
        api.clone(),
        "/api/v1/auth/password",
        json!({
            "current_password": "incorrect-current-password",
            "new_password": format!("Another-rotated-password-{suffix}")
        }),
        Some(&user_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = delete_json(
        api.clone(),
        &format!("/api/v1/targets/{}", uuid::Uuid::new_v4()),
        json!({"confirmed": true}),
        &user_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, logout_headers, _) = post_json_with_cookie_and_headers(
        api.clone(),
        "/api/v1/auth/logout",
        json!({}),
        &user_token,
        Some(&user_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cleared_cookies = logout_headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert_eq!(cleared_cookies.len(), 2);
    assert!(
        cleared_cookies
            .iter()
            .all(|value| value.contains("Max-Age=0"))
    );
    let (status, _) = get_json_with_cookie(api.clone(), "/api/v1/auth/session", &user_token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get_json(api.clone(), "/api/v1/auth/session", &user_token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, audit_after_user_actions) =
        get_json(api, "/api/v1/auth/audit?limit=50", &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    let user_events = audit_after_user_actions["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["actor_username"] == user_name)
        .collect::<Vec<_>>();
    assert!(
        audit_after_user_actions["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["actor_username"] == admin_name
                    && event["action"] == "user.created"
                    && event["status_code"] == StatusCode::CREATED.as_u16()
                    && event["details"]["state"] == "completed"
            })
    );
    assert!(
        user_events
            .iter()
            .any(|event| event["action"] == "auth.password_changed")
    );
    assert!(
        user_events
            .iter()
            .any(|event| event["action"] == "auth.logout")
    );
    assert!(user_events.iter().any(|event| {
        event["action"] == "auth.password_changed"
            && event["status_code"] == StatusCode::NO_CONTENT.as_u16()
            && event["details"]["state"] == "completed"
    }));
    assert!(user_events.iter().any(|event| {
        event["action"] == "target.deleted"
            && event["status_code"] == StatusCode::FORBIDDEN.as_u16()
            && event["details"]["state"] == "failed"
    }));
    assert!(user_events.iter().any(|event| {
        event["action"] == "http.get"
            && event["status_code"] == StatusCode::FORBIDDEN.as_u16()
            && event["details"]["path"] == "/api/v1/users"
    }));
    assert!(
        !audit_after_user_actions
            .to_string()
            .contains("Rotated-password")
    );

    sqlx::query("DELETE FROM user_audit_events WHERE actor_username = $1 OR actor_username = $2")
        .bind(&admin_name)
        .bind(&user_name)
        .execute(database.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth_users WHERE id = $1 OR id = $2")
        .bind(admin.id)
        .bind(user.id)
        .execute(database.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth_users WHERE id = $1")
        .bind(
            created_user["data"]["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        )
        .execute(database.pool())
        .await
        .unwrap();
    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

async fn get_json(api: axum::Router, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    response_json(api, request).await
}

async fn post_json(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method("POST").uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = builder
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json(api, request).await
}

async fn post_json_with_headers(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json_with_headers(api, request).await
}

async fn post_json_with_cookie(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    session: &str,
    csrf: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let (status, _, value) =
        post_json_with_cookie_and_headers(api, path, body, session, csrf).await;
    (status, value)
}

async fn patch_json_with_cookie(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    session: &str,
    csrf: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("PATCH")
        .uri(path)
        .header(
            "cookie",
            format!("lxcup_session={session}; lxcup_csrf={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json(api, request).await
}

async fn delete_json_with_cookie(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    session: &str,
    csrf: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .header(
            "cookie",
            format!("lxcup_session={session}; lxcup_csrf={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json(api, request).await
}

async fn post_json_with_cookie_and_headers(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    session: &str,
    csrf: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(
            "cookie",
            format!("lxcup_session={session}; lxcup_csrf={}", csrf.unwrap_or("")),
        )
        .header("content-type", "application/json");
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    let request = builder.body(Body::from(body.to_string())).unwrap();
    response_json_with_headers(api, request).await
}

async fn get_json_with_cookie(
    api: axum::Router,
    path: &str,
    session: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .uri(path)
        .header("cookie", format!("lxcup_session={session}"))
        .body(Body::empty())
        .unwrap();
    response_json(api, request).await
}

fn session_cookie(headers: &axum::http::HeaderMap) -> String {
    cookie_value(headers, "lxcup_session")
}

fn csrf_cookie(headers: &axum::http::HeaderMap) -> String {
    cookie_value(headers, "lxcup_csrf")
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> String {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix(&format!("{name}="))
                .and_then(|value| value.split(';').next())
                .map(str::to_owned)
        })
        .unwrap()
}

async fn delete_json(
    api: axum::Router,
    path: &str,
    body: serde_json::Value,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json(api, request).await
}

async fn response_json(
    api: axum::Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let (status, _, value) = response_json_with_headers(api, request).await;
    (status, value)
}

async fn response_json_with_headers(
    api: axum::Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let response = api.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or_default();
    (status, headers, value)
}
