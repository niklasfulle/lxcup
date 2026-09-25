use super::*;

#[tokio::test]
#[allow(unreachable_code)]
async fn removed_environment_api_is_not_available() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/environments")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "name": "pve-test",
                "endpoint": "https://pve.example.test/",
                "api_secret_ref": SecretId::new(),
                "ca_secret_ref": null
            })
            .to_string(),
        ))
        .unwrap();
    let response = router(state.clone()).oneshot(create).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    return;
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(json["data"]["endpoint"], "https://pve.example.test");
    assert!(!json.to_string().contains("token_secret"));

    let update = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/v1/environments/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"name": "pve-renamed"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(update)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let check = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/environments/{id}/check"))
        .body(Body::empty())
        .unwrap();
    let check_response = router(state.clone()).oneshot(check).await.unwrap();
    assert_eq!(check_response.status(), StatusCode::OK);
    let check_body = axum::body::to_bytes(check_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let check_json: serde_json::Value = serde_json::from_slice(&check_body).unwrap();
    assert_eq!(check_json["data"]["status"], "failed");

    let disable = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/environments/{id}/disable"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    let disable_response = router(state.clone()).oneshot(disable).await.unwrap();
    assert_eq!(disable_response.status(), StatusCode::OK);
    let delete = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/v1/environments/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state).oneshot(delete).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
#[allow(unreachable_code)]
async fn removed_container_action_api_is_not_available() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let stop_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/101/actions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "action": "shutdown",
                    "idempotency_key": "shutdown-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap()
    };
    let first = router(state.clone()).oneshot(stop_request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::NOT_FOUND);
    return;
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let task_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["status"], "queued");

    let duplicate = router(state.clone()).oneshot(stop_request()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);

    let start = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/actions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "action": "start",
                "idempotency_key": "start-1",
                "confirmed": false
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone()).oneshot(start).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    let status = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/container-actions/{task_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
}

#[tokio::test]
async fn secret_api_redacts_values_and_records_lifecycle_audit() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/secrets")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "name": "test-connection-token",
                "kind": "generic",
                "scope": {"type": "global"},
                "value": "never-return-this-value"
            })
            .to_string(),
        ))
        .unwrap();
    let response = router(state.clone()).oneshot(create).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = json["data"]["metadata"]["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!json.to_string().contains("never-return-this-value"));

    let rotate_without_confirmation = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/secrets/{id}/rotate"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"value":"rotated-value","confirmed":false}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(rotate_without_confirmation)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let revoke = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/secrets/{id}/revoke"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state.clone())
            .oneshot(revoke)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let audit = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/secrets/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit_body = axum::body::to_bytes(audit.into_body(), usize::MAX)
        .await
        .unwrap();
    let audit_json: serde_json::Value = serde_json::from_slice(&audit_body).unwrap();
    assert_eq!(audit_json["data"][0]["action"], "created");
    assert!(!audit_json.to_string().contains("never-return-this-value"));
}

#[test]
fn invalid_ids_are_rejected() {
    assert!(parse_container_id("0").is_err());
    assert!(parse_uuid("not-an-id", "plan id").is_err());
}

#[tokio::test]
async fn readiness_reports_degraded_targets_and_metrics_keep_operational_signals_visible() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let mut target = Target::new(
        "disabled-healthcheck-target",
        TargetKind::LinuxServer,
        "192.0.2.40",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.state = lxcup_core::TargetState::Disabled;
    state.store.write().await.targets.push(target);

    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let readiness: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(readiness["checks"]["targets"], "degraded");

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics = String::from_utf8(body.to_vec()).unwrap();
    assert!(metrics.contains("lxcup_ansible_jobs_queued 0"));
    assert!(metrics.contains("lxcup_ansible_jobs_failed 0"));
    assert!(metrics.contains("lxcup_ansible_queue_age_seconds 0"));
    assert!(metrics.contains("lxcup_worker_heartbeat_age_seconds -1"));
    assert!(metrics.contains("lxcup_worker_available 0"));
}

#[tokio::test]
async fn metrics_endpoint_reports_worker_and_queue_signals_without_a_database() {
    let response = router(ApiState::new().with_auth_config(AuthConfig::disabled()))
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics = String::from_utf8(body.to_vec()).unwrap();
    assert!(metrics.contains("lxcup_ansible_jobs_queued 0"));
    assert!(metrics.contains("lxcup_ansible_jobs_failed 0"));
    assert!(metrics.contains("lxcup_ansible_queue_age_seconds 0"));
    assert!(metrics.contains("lxcup_worker_heartbeat_age_seconds -1"));
    assert!(metrics.contains("lxcup_worker_available 0"));
}
