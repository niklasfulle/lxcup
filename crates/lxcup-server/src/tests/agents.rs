use super::*;

#[tokio::test]
async fn compose_image_update_requires_and_uses_a_fresh_successful_check() {
    use axum::routing::post;
    use lxcup_secrets::{CreateSecret, SecretStore};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::RwLock;

    let checked_status = Arc::new(RwLock::new(lxcup_agent::DockerImageUpdateStatus::Current));
    let apply_count = Arc::new(AtomicUsize::new(0));
    let id = "aaaaaaaaaaaa".to_owned();
    let expected_digest = format!("sha256:{}", "b".repeat(64));
    let container = lxcup_agent::DockerContainerInfo {
        id: id.clone(),
        name: "web".to_owned(),
        image: "nginx:stable".to_owned(),
        state: "running".to_owned(),
        status: "Up".to_owned(),
        ports: Vec::new(),
        started_at: None,
        created_at: None,
        image_id: Some(format!("sha256:{}", "a".repeat(64))),
        restart_count: None,
        health: None,
        oom_killed: None,
        labels: vec!["com.docker.compose.service=web".to_owned()],
    };
    let discovery = lxcup_agent::DockerDiscovery {
        available: true,
        reason: None,
        collected_at: chrono::Utc::now(),
        containers: vec![container.clone()],
    };
    let mock_agent = axum::Router::new()
        .route(
            "/docker/containers",
            axum::routing::get(move || {
                let discovery = discovery.clone();
                async move { axum::Json(discovery) }
            }),
        )
        .route(
            "/docker/containers/image-update-check",
            post({
                let status = checked_status.clone();
                let expected_digest = expected_digest.clone();
                let id = id.clone();
                let image = container.image.clone();
                move || {
                    let status = status.clone();
                    let expected_digest = expected_digest.clone();
                    let id = id.clone();
                    let image = image.clone();
                    async move {
                        axum::Json(lxcup_agent::DockerImageUpdateResult {
                            container_id: id,
                            image,
                            current_image_id: Some(format!("sha256:{}", "a".repeat(64))),
                            remote_image_id: Some(expected_digest),
                            status: *status.read().await,
                            reason: None,
                            checked_at: chrono::Utc::now(),
                        })
                    }
                }
            }),
        )
        .route(
            "/docker/containers/image-update-apply",
            post({
                let count = apply_count.clone();
                let id = id.clone();
                let expected_digest = expected_digest.clone();
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    let id = id.clone();
                    let expected_digest = expected_digest.clone();
                    async move {
                        axum::Json(lxcup_agent::DockerImageUpdateApplyResult {
                            container_id: id,
                            image: "nginx:stable".to_owned(),
                            compose_project: "shop".to_owned(),
                            compose_service: "web".to_owned(),
                            image_id: expected_digest,
                            completed_at: chrono::Utc::now(),
                        })
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 8090))
        .await
        .expect("test agent port should be available");
    let mock_agent_task = tokio::spawn(async move {
        axum::serve(listener, mock_agent).await.unwrap();
    });

    let secret_store = InMemorySecretStore::default();
    let agent_secret = secret_store
        .create(CreateSecret {
            name: "compose-update-agent".to_owned(),
            kind: SecretKind::AgentToken,
            scope: SecretScope::Global,
            value: SecretValue::new("compose-update-token").unwrap(),
        })
        .unwrap();
    let mut target = Target::new(
        "compose-target",
        TargetKind::Lxc,
        "127.0.0.1",
        TargetTransport::Ssh,
        SecretId::new(),
        agent_secret.metadata.id,
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    state.store.write().await.targets.push(target);

    let apply_request = || {
        Request::builder()
            .method(Method::POST)
            .uri(format!(
                "/api/v1/targets/{}/docker/containers/{id}/image-update-apply",
                target_id.as_uuid()
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "confirmed": true,
                    "expected_remote_image_id": expected_digest,
                })
                .to_string(),
            ))
            .unwrap()
    };
    let api = router(state.clone());
    let stale = api.clone().oneshot(apply_request()).await.unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(apply_count.load(Ordering::SeqCst), 0);

    *checked_status.write().await = lxcup_agent::DockerImageUpdateStatus::UpdateAvailable;
    let applied = api.oneshot(apply_request()).await.unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
    assert_eq!(apply_count.load(Ordering::SeqCst), 1);
    let saved = state
        .store
        .read()
        .await
        .target_docker_inventories
        .get(&target_id)
        .cloned()
        .unwrap();
    assert_eq!(saved.containers[0].id, id);
    mock_agent_task.abort();
}

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
        version: "0.3.1".to_owned(),
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
async fn docker_lifecycle_route_requires_confirmation_before_accessing_an_agent() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/targets/{}/docker/containers/aaaaaaaaaaaa/action",
                    uuid::Uuid::new_v4()
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"action":"restart","confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "confirmation_required");
}

#[tokio::test]
async fn docker_image_update_check_rejects_unvalidated_container_ids() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/targets/{}/docker/containers/not-an-id/image-update-check",
                    uuid::Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "invalid_container_id");
}

#[tokio::test]
async fn compose_image_update_requires_confirmation_before_agent_access() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/targets/{}/docker/containers/aaaaaaaaaaaa/image-update-apply",
                    uuid::Uuid::new_v4()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"confirmed":false,"expected_remote_image_id":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "confirmation_required");
}

#[tokio::test]
async fn compose_image_update_rejects_an_unvalidated_expected_digest() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/targets/{}/docker/containers/aaaaaaaaaaaa/image-update-apply",
                    uuid::Uuid::new_v4()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"confirmed":true,"expected_remote_image_id":"--help"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "invalid_update_request");
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
            version: "0.3.1".to_owned(),
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
                    collected_at: now - chrono::Duration::seconds(61),
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
        docker_telemetry: lxcup_agent::DockerTelemetryWindow {
            samples: vec![lxcup_agent::DockerTelemetrySample {
                collected_at: now,
                container_id: "aaaaaaaaaaaa".to_owned(),
                cpu_basis_points: Some(1250),
                memory_basis_points: Some(410),
                memory_used_bytes: Some(44_145_050),
                memory_limit_bytes: Some(1_073_741_824),
            }],
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
    assert_eq!(telemetry_json["data"]["partial"], true);
    assert_eq!(telemetry_json["data"]["missing_samples"], 0);

    let docker_telemetry_response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/targets/{}/docker/telemetry",
                    target_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(docker_telemetry_response.status(), StatusCode::OK);
    let docker_telemetry_body =
        axum::body::to_bytes(docker_telemetry_response.into_body(), usize::MAX)
            .await
            .unwrap();
    let docker_telemetry_json: serde_json::Value =
        serde_json::from_slice(&docker_telemetry_body).unwrap();
    assert_eq!(
        docker_telemetry_json["data"]["samples"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        docker_telemetry_json["data"]["samples"][0]["memory_basis_points"],
        410
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
    assert_eq!(targets_json["data"][0]["agent_version"], "0.3.1");
    assert_eq!(
        targets_json["data"][0]["latest_agent_version"],
        env!("CARGO_PKG_VERSION")
    );
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
async fn telemetry_alert_endpoint_returns_sustained_load_with_resource_context() {
    let mut target = Target::new(
        "alert-target",
        TargetKind::Lxc,
        "192.0.2.44",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let target_id = target.id;
    let now = chrono::Utc::now();
    let samples = (0..=61)
        .map(|index| lxcup_agent::SystemTelemetrySample {
            collected_at: now - chrono::Duration::seconds(index * 5),
            cpu_basis_points: Some(9_300),
            memory_basis_points: Some(2_000),
            storage_basis_points: Some(4_000),
            load_1_milli: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            process_count: None,
        })
        .collect();
    let heartbeat = lxcup_agent::AgentHeartbeat {
        target_id: target_id.as_uuid(),
        info: lxcup_agent::AgentInfo {
            agent_id: "alert-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Linux,
            hostname: "alert-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        },
        metrics: lxcup_agent::AgentMetrics {
            collected_at: now,
            commands_total: 0,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: lxcup_agent::SystemTelemetryWindow {
            samples,
            partial: false,
        },
        docker_telemetry: Default::default(),
    };
    let state = ApiState::new();
    {
        let mut store = state.store.write().await;
        store.targets.push(target);
        store.agent_reports.insert(target_id, heartbeat);
    }

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/telemetry-alerts")
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
    let alert = &json["data"][0];
    assert_eq!(alert["target_name"], "alert-target");
    assert_eq!(alert["target_address"], "192.0.2.44");
    assert_eq!(alert["metric"], "CPU");
    assert_eq!(alert["severity"], "warning");
    assert_eq!(alert["value_basis_points"], 9_300);
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
