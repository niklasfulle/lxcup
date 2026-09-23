use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lxcup_secrets::CreateSecret;
use tower::ServiceExt;

#[tokio::test]
async fn nodes_endpoint_returns_versioned_envelope() {
    let state = ApiState::new();
    state
        .replace_nodes(vec![Node::new("pve01", "https://pve01:8006").unwrap()])
        .await;
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn enrollment_endpoint_is_idempotent_and_queryable() {
    let state = ApiState::new();
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let create_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": 101,
                    "idempotency_key": "request-1"
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let enrollment_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["state"], "requested");

    let second = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["data"]["id"], enrollment_id);

    let get = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/enrollments/{enrollment_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
}

#[tokio::test]
async fn enrollment_rejects_key_reuse_and_unknown_containers() {
    let state = ApiState::new();
    let containers = [101, 102]
        .into_iter()
        .map(|id| {
            Container::new(
                ContainerId::new(id),
                NodeId::new(),
                format!("test-lxc-{id}"),
                lxcup_core::OperatingSystem::Debian,
                lxcup_core::ContainerStatus::Running,
            )
            .unwrap()
        })
        .collect();
    state.replace_containers(containers).await;

    let request = |container_id, key| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": container_id,
                    "idempotency_key": key
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(request(101, "request-1"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);

    let reused = router(state.clone())
        .oneshot(request(102, "request-1"))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);

    let unknown = router(state)
        .oneshot(request(999, "request-2"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ansible_job_endpoint_accepts_healthcheck_and_is_idempotent() {
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
    let request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/ansible/jobs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "operation": "health_check",
                    "container_id": 101,
                    "mode": "check",
                    "parameters": {"operation": "health_check"},
                    "idempotency_key": "health-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let job_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["status"], "queued");

    let duplicate = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_body = axum::body::to_bytes(duplicate.into_body(), usize::MAX)
        .await
        .unwrap();
    let duplicate_json: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
    assert_eq!(duplicate_json["data"]["id"], job_id);

    let events = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);

    let status = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
}

#[tokio::test]
async fn configured_auth_protects_api_but_not_health() {
    let state = ApiState::new().with_auth_config(
        AuthConfig::disabled()
            .with_tokens(Some("viewer".to_owned()), Some("operator".to_owned()), None)
            .required(true),
    );
    let api_response = router(state.clone())
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api_response.status(), StatusCode::UNAUTHORIZED);

    let health_response = router(state)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn agent_registration_exposes_health_and_metrics() {
    let agent_token = "test-agent-token";
    let agent_info = lxcup_agent::AgentInfo {
        agent_id: "test-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Linux,
        hostname: "test-lxc".to_owned(),
        version: "0.1.0".to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(agent_info, agent_token)),
        )
        .await
        .unwrap();
    });

    let secret_store = InMemorySecretStore::default();
    let agent_secret = secret_store
        .create(CreateSecret {
            name: "test-agent".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Container(ContainerId::new(101)),
            value: lxcup_core::SecretValue::new(agent_token).unwrap(),
        })
        .unwrap();
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let registration = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/agent")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "endpoint": format!("http://{address}"),
                "secret_ref": agent_secret.metadata.id,
            })
            .to_string(),
        ))
        .unwrap();
    let registration_response = router(state.clone()).oneshot(registration).await.unwrap();
    assert_eq!(registration_response.status(), StatusCode::CREATED);

    let health_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/101/agent/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), StatusCode::OK);
    let health_body = axum::body::to_bytes(health_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let health_json: serde_json::Value = serde_json::from_slice(&health_body).unwrap();
    assert_eq!(health_json["data"]["healthy"], true);
    assert_eq!(health_json["data"]["info"]["agent_id"], "test-agent");

    let metrics_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/101/agent/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics_response.status(), StatusCode::OK);
    let metrics_body = axum::body::to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_json: serde_json::Value = serde_json::from_slice(&metrics_body).unwrap();
    assert_eq!(metrics_json["data"]["commands_total"], 0);
    assert_eq!(metrics_json["data"]["commands_failed"], 0);

    let revoke = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/containers/101/agent/revoke")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"confirmed":true}"#))
        .unwrap();
    assert_eq!(
        router(state).oneshot(revoke).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    agent_task.abort();
}

#[tokio::test]
async fn environment_api_keeps_secret_values_out_of_responses_and_supports_lifecycle() {
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
    assert_eq!(response.status(), StatusCode::CREATED);
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
async fn container_action_api_validates_status_and_deduplicates_tasks() {
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
    assert_eq!(first.status(), StatusCode::ACCEPTED);
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
                "name": "test-proxmox-token",
                "kind": "proxmox_api_token",
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
