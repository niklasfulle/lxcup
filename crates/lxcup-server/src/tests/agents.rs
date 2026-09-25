use super::*;

#[tokio::test]
async fn configured_auth_protects_api_but_not_health() {
    let state = ApiState::new().with_auth_config(
        AuthConfig::disabled()
            .with_tokens(
                Some("viewer-secret".to_owned()),
                Some("operator-secret".to_owned()),
                None,
            )
            .required(true)
            .with_token_ttl(std::time::Duration::from_secs(3600)),
    );
    let api_response = router(state.clone())
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api_response.status(), StatusCode::UNAUTHORIZED);

    let viewer_session = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/session")
                .header("authorization", "Bearer viewer-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_session.status(), StatusCode::OK);
    let session_body = axum::body::to_bytes(viewer_session.into_body(), usize::MAX)
        .await
        .unwrap();
    let session: serde_json::Value = serde_json::from_slice(&session_body).unwrap();
    assert_eq!(session["data"]["role"], "viewer");
    assert!(session["data"]["expires_in_seconds"].is_number());
    assert!(!session.to_string().contains("viewer-secret"));

    let viewer_mutation = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/targets")
                .header("authorization", "Bearer viewer-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_mutation.status(), StatusCode::FORBIDDEN);

    let viewer_logout = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/logout")
                .header("authorization", "Bearer viewer-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_logout.status(), StatusCode::NO_CONTENT);

    let unauthenticated_events = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_events.status(), StatusCode::UNAUTHORIZED);

    let operator_config_attempt = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/targets")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!matches!(
        operator_config_attempt.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));

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
async fn worker_availability_reports_unavailable_without_a_recent_heartbeat() {
    let response = router(ApiState::new().with_auth_config(AuthConfig::disabled()))
        .oneshot(
            Request::builder()
                .uri("/api/v1/ansible/worker-availability")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["available"], false);
}

#[tokio::test]
async fn agent_registration_exposes_health_and_metrics() {
    let agent_token = "test-agent-token";
    let agent_info = lxcup_agent::AgentInfo {
        agent_id: "test-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Linux,
        hostname: "test-lxc".to_owned(),
        version: "0.2.0".to_owned(),
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
async fn agent_heartbeat_updates_target_state_without_activity_event() {
    let agent_token = "heartbeat-token";
    let secret_store = InMemorySecretStore::default();
    let secret = secret_store
        .create(CreateSecret {
            name: "heartbeat-agent".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new(agent_token).unwrap(),
        })
        .unwrap();
    let target = Target::new(
        "heartbeat-target",
        TargetKind::LinuxServer,
        "192.0.2.77",
        TargetTransport::Ssh,
        SecretId::new(),
        secret.metadata.id,
    )
    .unwrap();
    let target_id = target.id;
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    state.store.write().await.targets.push(target);
    let mut events = state.events.subscribe();

    let now = chrono::Utc::now();
    let heartbeat = lxcup_agent::AgentHeartbeat {
        target_id: target_id.as_uuid(),
        info: lxcup_agent::AgentInfo {
            agent_id: "heartbeat-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Linux,
            hostname: "heartbeat-host".to_owned(),
            version: "0.2.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        },
        metrics: lxcup_agent::AgentMetrics {
            collected_at: now,
            commands_total: 1,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: lxcup_agent::SystemTelemetryWindow {
            samples: vec![
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now,
                    cpu_basis_points: Some(4_200),
                    memory_basis_points: Some(5_000),
                    storage_basis_points: Some(6_000),
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now - chrono::Duration::seconds(1),
                    cpu_basis_points: Some(3_500),
                    memory_basis_points: Some(4_500),
                    storage_basis_points: Some(5_500),
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now,
                    cpu_basis_points: Some(10_001),
                    memory_basis_points: None,
                    storage_basis_points: None,
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
                lxcup_agent::SystemTelemetrySample {
                    collected_at: now - chrono::Duration::seconds(31),
                    cpu_basis_points: None,
                    memory_basis_points: None,
                    storage_basis_points: None,
                    load_1_milli: None,
                    network_rx_bytes: None,
                    network_tx_bytes: None,
                    process_count: None,
                },
            ],
            partial: false,
        },
    };
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/agents/heartbeat")
                .header("authorization", format!("Bearer {agent_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&heartbeat).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let store = state.store.read().await;
    assert_eq!(store.targets[0].state, TargetState::Managed);
    assert!(store.agent_reports.contains_key(&target_id));
    let received_telemetry = &store.agent_reports[&target_id].telemetry;
    assert!(received_telemetry.partial);
    assert_eq!(received_telemetry.samples.len(), 2);
    assert!(
        received_telemetry.samples[0].collected_at < received_telemetry.samples[1].collected_at
    );
    drop(store);

    let telemetry_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/targets/{}/telemetry", target_id.as_uuid()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(telemetry_response.status(), StatusCode::OK);
    let telemetry_body = axum::body::to_bytes(telemetry_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let telemetry_json: serde_json::Value = serde_json::from_slice(&telemetry_body).unwrap();
    assert_eq!(
        telemetry_json["data"]["samples"].as_array().unwrap().len(),
        2
    );

    let targets = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(targets.status(), StatusCode::OK);
    let targets_body = axum::body::to_bytes(targets.into_body(), usize::MAX)
        .await
        .unwrap();
    let targets_json: serde_json::Value = serde_json::from_slice(&targets_body).unwrap();
    assert_eq!(targets_json["data"][0]["agent_version"], "0.2.0");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn package_inventory_endpoint_reports_an_uncollected_target_without_packages() {
    let target = Target::new(
        "inventory-target",
        TargetKind::LinuxServer,
        "192.0.2.99",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let state = ApiState::new();
    state.store.write().await.targets.push(target);

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/targets/{}/package-inventory",
                    target_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["status"], "not_collected");
    assert_eq!(json["data"]["packages"], serde_json::json!([]));
    assert!(json["data"]["collected_at"].is_null());
}

#[tokio::test]
async fn agent_endpoints_surface_unavailable_clients_and_confirmation_errors() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container_id = ContainerId::new(505);
    let container = Container::new(
        container_id,
        NodeId::new(),
        "unavailable-agent",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new("http://127.0.0.1:1", "token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(container_id, RegisteredAgent { client });

    for uri in [
        "/api/v1/containers/505/agent/health",
        "/api/v1/containers/505/agent/metrics",
    ] {
        let response = router(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    let not_confirmed = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_confirmed.status(), StatusCode::BAD_REQUEST);

    let revoked = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let second_revoke = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/505/agent/revoke")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_revoke.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn agent_registration_requires_a_secret_reference_and_known_container() {
    let secret_store = InMemorySecretStore::default();
    let agent_secret = secret_store
        .create(CreateSecret {
            name: "agent-config-test".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new("token").unwrap(),
        })
        .unwrap();
    let ca_secret = secret_store
        .create(CreateSecret {
            name: "agent-ca-test".to_owned(),
            kind: lxcup_core::SecretKind::Generic,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new("not-a-pem").unwrap(),
        })
        .unwrap();
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    let container = Container::new(
        ContainerId::new(506),
        NodeId::new(),
        "secret-required-agent",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let missing_secret = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/506/agent")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"endpoint":"http://127.0.0.1:1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_secret.status(), StatusCode::BAD_REQUEST);

    let invalid_ca = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/506/agent")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "endpoint": "http://127.0.0.1:1",
                        "secret_ref": agent_secret.metadata.id,
                        "ca_secret_ref": ca_secret.metadata.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_ca.status(), StatusCode::BAD_GATEWAY);

    let missing_container = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/507/agent")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "endpoint": "http://127.0.0.1:1",
                        "secret_ref": SecretId::new()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_container.status(), StatusCode::NOT_FOUND);
}
