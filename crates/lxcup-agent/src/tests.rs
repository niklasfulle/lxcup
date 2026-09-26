use super::*;
use axum::{body::Body, http::Request, response::IntoResponse, routing::get};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[test]
fn config_redacts_tokens_and_requires_safe_urls() {
    assert!(AgentClientConfig::new("http://agent.example", "token").is_err());
    assert!(matches!(
        AgentClientConfig::new("https://agent.example", "  "),
        Err(AgentConfigError::EmptyToken)
    ));
    let config = AgentClientConfig::new("https://agent.example", "secret").unwrap();
    assert!(!format!("{config:?}").contains("secret"));
    assert!(
        AgentClientConfig::new("https://agent.example", "secret")
            .unwrap()
            .with_root_certificate_pem(b"private-ca")
            .root_certificate_pem
            .is_some()
    );
}

#[test]
fn private_network_http_config_accepts_only_literal_private_addresses() {
    assert!(
        AgentClientConfig::new_for_private_network_http("http://192.168.1.20:8090", "token")
            .is_ok()
    );
    assert!(
        AgentClientConfig::new_for_private_network_http("http://[fd00::20]:8090", "token").is_ok()
    );
    assert!(
        AgentClientConfig::new_for_private_network_http("http://agent.local:8090", "token")
            .is_err()
    );
    assert!(
        AgentClientConfig::new_for_private_network_http("http://203.0.113.20:8090", "token")
            .is_err()
    );
    assert!(
        AgentClientConfig::new_for_private_network_http("https://192.168.1.20:8090", "token")
            .is_err()
    );
}

#[test]
fn package_validation_blocks_option_injection() {
    assert!(safe_package("openssl"));
    assert!(!safe_package("--download-only"));
    assert!(!safe_package("openssl; reboot"));
}

#[test]
fn apt_output_is_normalized_to_the_agent_contract() {
    let output =
        normalize_apt_list("Listing...\nopenssl/stable-security 3.1 amd64 [upgradable from: 3.0]");
    assert!(output.contains("Package: openssl"));
    assert!(output.contains("Security: yes"));
    assert!(output.contains("Installed: 3.0"));
}

#[test]
fn docker_inventory_parser_keeps_only_complete_rows() {
    let containers = parse_docker_containers(
        "a1\tapi\tghcr.io/acme/api:1\trunning\tUp 2 hours\t80/tcp\t2026-01-01\tapp=api,secret=value\ninvalid",
    );
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].name, "api");
    assert_eq!(containers[0].ports, ["80/tcp"]);
    assert_eq!(containers[0].labels, ["app=api"]);
}

#[test]
fn package_inventory_parsers_normalize_linux_and_windows_fixtures() {
    let linux = parse_dpkg_packages("curl\t8.5.0-2\tamd64\ninvalid");
    assert_eq!(linux.len(), 1);
    assert_eq!(linux[0].source.as_deref(), Some("dpkg"));
    let windows =
        parse_windows_packages(r#"[{"Name":"7zip","Version":"24.0","ProviderName":"Programs"}]"#);
    assert_eq!(windows[0].name, "7zip");
    assert_eq!(windows[0].source.as_deref(), Some("Programs"));
}

#[test]
fn telemetry_buffer_keeps_a_bounded_overlapping_sixty_second_window() {
    let mut buffer = TelemetryBuffer::default();
    let now = Utc::now();
    buffer.record(SystemTelemetrySample {
        collected_at: now - chrono::Duration::seconds(61),
        cpu_basis_points: None,
        memory_basis_points: None,
        storage_basis_points: None,
        load_1_milli: None,
        network_rx_bytes: None,
        network_tx_bytes: None,
        process_count: None,
    });
    for offset in 0..40 {
        buffer.record(SystemTelemetrySample {
            collected_at: now - chrono::Duration::seconds(59)
                + chrono::Duration::milliseconds(offset * 500),
            cpu_basis_points: Some(5000),
            memory_basis_points: Some(4000),
            storage_basis_points: None,
            load_1_milli: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            process_count: None,
        });
    }
    let window = buffer.window();
    assert_eq!(window.samples.len(), TelemetryBuffer::MAX_SAMPLES);
    assert!(window.partial);
    assert!(window.samples.iter().all(|sample| {
        sample.collected_at
            >= Utc::now() - chrono::Duration::seconds(TelemetryBuffer::WINDOW_SECONDS)
    }));
    assert!(
        window
            .samples
            .windows(2)
            .all(|pair| pair[0].collected_at <= pair[1].collected_at)
    );
    let serialized = serde_json::to_vec(&window).unwrap();
    let restored: SystemTelemetryWindow = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(restored, window);
}

#[tokio::test]
async fn authenticated_health_command_is_idempotent_and_updates_metrics() {
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "test-agent".to_owned(),
            platform: if cfg!(windows) {
                AgentPlatform::Windows
            } else {
                AgentPlatform::Linux
            },
            hostname: "test-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "agent-token",
    );
    let app = agent_router(state);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let request = serde_json::json!({
        "action": "health",
        "packages": [],
        "idempotency_key": "health-check-1"
    })
    .to_string();
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/command")
                .header("authorization", "Bearer agent-token")
                .header("content-type", "application/json")
                .body(Body::from(request.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/command")
                .header("authorization", "Bearer agent-token")
                .header("content-type", "application/json")
                .body(Body::from(request))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(first_json["request_id"], second_json["request_id"]);

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("authorization", "Bearer agent-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics_body = axum::body::to_bytes(metrics.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_json: serde_json::Value = serde_json::from_slice(&metrics_body).unwrap();
    assert_eq!(metrics_json["commands_total"], 1);
    assert_eq!(metrics_json["commands_failed"], 0);
}

#[tokio::test]
async fn inventory_routes_enforce_auth_and_report_platform_support() {
    let linux_state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "inventory-token",
    );
    let linux = agent_router(linux_state);

    let unauthorized = linux
        .clone()
        .oneshot(
            Request::builder()
                .uri("/packages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    for path in ["/packages", "/docker/containers"] {
        let response = linux
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", "Bearer inventory-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().is_success()
                || matches!(
                    response.status(),
                    StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
                )
        );
    }

    let windows = agent_router(LocalAgentState::new(
        AgentInfo {
            agent_id: "windows-agent".to_owned(),
            platform: AgentPlatform::Windows,
            hostname: "windows-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "inventory-token",
    ));
    let docker = windows
        .clone()
        .oneshot(
            Request::builder()
                .uri("/docker/containers")
                .header("authorization", "Bearer inventory-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(docker.status(), StatusCode::OK);
    let body = axum::body::to_bytes(docker.into_body(), usize::MAX)
        .await
        .unwrap();
    let discovery: DockerDiscovery = serde_json::from_slice(&body).unwrap();
    assert!(!discovery.available);
    assert_eq!(discovery.reason.as_deref(), Some("unsupported_platform"));
    let packages = windows
        .oneshot(
            Request::builder()
                .uri("/packages")
                .header("authorization", "Bearer inventory-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        packages.status(),
        StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    ));
}

#[tokio::test]
async fn remote_client_retries_server_errors_and_classifies_terminal_failures() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/health",
            get(|| async {
                Json(AgentHealth {
                    healthy: true,
                    info: AgentInfo {
                        agent_id: "remote-agent".to_owned(),
                        platform: AgentPlatform::Linux,
                        hostname: "remote-host".to_owned(),
                        version: "0.3.1".to_owned(),
                        protocol_version: PROTOCOL_VERSION.to_owned(),
                    },
                    metrics: AgentMetrics {
                        collected_at: Utc::now(),
                        commands_total: 0,
                        commands_failed: 0,
                        last_command_at: None,
                    },
                })
            }),
        )
        .route(
            "/metrics",
            get({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                            StatusCode::SERVICE_UNAVAILABLE.into_response()
                        } else {
                            Json(AgentMetrics {
                                collected_at: Utc::now(),
                                commands_total: 3,
                                commands_failed: 1,
                                last_command_at: None,
                            })
                            .into_response()
                        }
                    }
                }
            }),
        )
        .route(
            "/docker/containers",
            get(|| async { (StatusCode::BAD_REQUEST, "invalid response").into_response() }),
        )
        .route(
            "/packages",
            get(|| async { StatusCode::UNAUTHORIZED.into_response() }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = AgentClientConfig::new(endpoint, "remote-token")
        .unwrap()
        .with_retry_policy(1, Duration::ZERO);
    let client = AgentClient::new(config.clone()).unwrap();
    assert!(!format!("{client:?}").contains("remote-token"));

    assert!(client.health().await.unwrap().healthy);
    let metrics = client.metrics().await.unwrap();
    assert_eq!(metrics.commands_total, 3);
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    let api_error = client.docker_containers().await.unwrap_err();
    assert_eq!(api_error.code(), "agent_api_error");
    assert!(!api_error.is_retryable());
    let unauthorized = client.package_inventory().await.unwrap_err();
    assert_eq!(unauthorized.code(), "agent_unauthorized");

    let client_build_error = AgentClient::new(config.with_root_certificate_pem(
        b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----",
    ))
    .unwrap_err();
    assert_eq!(client_build_error.code(), "agent_client_error");
    assert!(!client_build_error.is_retryable());
    let invalid_key = AgentCommandRequest {
        action: AgentAction::Health,
        packages: Vec::new(),
        idempotency_key: "  ".to_owned(),
    };
    assert!(matches!(
        client.command(&invalid_key).await,
        Err(AgentError::InvalidRequest(_))
    ));
    let unsafe_package = AgentCommandRequest {
        action: AgentAction::Apply,
        packages: vec!["--force".to_owned()],
        idempotency_key: "safe-key".to_owned(),
    };
    assert!(matches!(
        client.command(&unsafe_package).await,
        Err(AgentError::InvalidRequest(_))
    ));
    assert_eq!(
        AgentError::InvalidRequest("bad request").code(),
        "agent_invalid_request"
    );
    assert_eq!(
        AgentError::Decode(reqwest::Client::new().get("://").send().await.unwrap_err(),).code(),
        "agent_invalid_response"
    );

    server.abort();
    let transport_error = client.health().await.unwrap_err();
    assert_eq!(transport_error.code(), "agent_unreachable");
    assert!(transport_error.is_retryable());
}

#[tokio::test]
async fn agent_state_exposes_metric_and_recent_telemetry_snapshots() {
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "telemetry-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "telemetry-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "telemetry-token",
    );
    let sample = SystemTelemetrySample {
        collected_at: Utc::now(),
        cpu_basis_points: Some(1200),
        memory_basis_points: Some(3400),
        storage_basis_points: Some(5600),
        load_1_milli: Some(900),
        network_rx_bytes: Some(12),
        network_tx_bytes: Some(34),
        process_count: Some(5),
    };
    state.record_telemetry(sample.clone()).await;
    assert_eq!(state.telemetry_window().await.samples, [sample]);
    assert_eq!(state.metrics_snapshot().await.commands_total, 0);
}

#[tokio::test]
async fn command_endpoint_rejects_bad_requests_and_evicts_full_idempotency_cache() {
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "cache-agent".to_owned(),
            platform: if cfg!(windows) {
                AgentPlatform::Windows
            } else {
                AgentPlatform::Linux
            },
            hostname: "cache-host".to_owned(),
            version: "0.3.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "cache-token",
    );
    let app = agent_router(state.clone());
    let send = |app: Router, key: &str, packages: Vec<String>, authorized: bool| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/command")
            .header("content-type", "application/json");
        if authorized {
            builder = builder.header("authorization", "Bearer cache-token");
        }
        app.oneshot(
            builder
                .body(Body::from(
                    serde_json::json!({
                        "action": "health",
                        "packages": packages,
                        "idempotency_key": key
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
    };
    let unauthorized = send(app.clone(), "key", Vec::new(), false).await.unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let empty_key = send(app.clone(), " ", Vec::new(), true).await.unwrap();
    assert_eq!(empty_key.status(), StatusCode::BAD_REQUEST);
    let option = send(app.clone(), "option", vec!["--force".to_owned()], true)
        .await
        .unwrap();
    assert_eq!(option.status(), StatusCode::BAD_REQUEST);

    {
        let mut results = state.results.lock().await;
        for index in 0..512 {
            results.insert(
                format!("cached-{index}"),
                AgentCommandResponse {
                    request_id: Uuid::new_v4(),
                    success: true,
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    reboot_required: false,
                    duration_ms: 0,
                },
            );
        }
    }
    let response = send(app, "new-command", Vec::new(), true).await.unwrap();
    assert!(response.status().is_success());
    let results = state.results.lock().await;
    assert_eq!(results.len(), 1);
    assert!(results.contains_key("new-command"));
}
