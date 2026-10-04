use super::*;
use crate::docker::DockerCommandRunner as _;
use crate::parsers::{
    docker_failure_reason, parse_docker_containers, parse_docker_inspect_metadata,
};
use axum::{Json, Router, body::Body, http::Request, response::IntoResponse, routing::get};
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;

#[derive(Default)]
pub(crate) struct ScriptedDockerRunner {
    pub(crate) outputs: Mutex<std::collections::VecDeque<crate::docker::DockerCommandOutput>>,
    commands: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
}

impl crate::docker::DockerCommandRunner for ScriptedDockerRunner {
    fn output<'a>(
        &'a self,
        args: &'a [String],
        working_directory: Option<&'a Path>,
    ) -> Pin<Box<dyn Future<Output = io::Result<crate::docker::DockerCommandOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            self.commands
                .lock()
                .unwrap()
                .push((args.to_vec(), working_directory.map(Path::to_path_buf)));
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| io::Error::other("no scripted Docker result"))
        })
    }
}

pub(crate) fn docker_result(
    success: bool,
    stdout: impl Into<Vec<u8>>,
) -> crate::docker::DockerCommandOutput {
    crate::docker::DockerCommandOutput {
        success,
        stdout: stdout.into(),
        stderr: Vec::new(),
    }
}

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
        "aaaaaaaaaaaa\tapi\tghcr.io/acme/api:1\trunning\tUp 2 hours\t80/tcp\tapp=api,secret=value\ninvalid",
    );
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].name, "api");
    assert_eq!(containers[0].ports, ["80/tcp"]);
    assert_eq!(containers[0].labels, ["app=api"]);
    assert_eq!(containers[0].started_at, None);
    assert_eq!(containers[0].created_at, None);
}

#[test]
fn docker_stats_parser_normalizes_memory_and_discards_bad_rows() {
    let now = chrono::Utc::now();
    let samples = crate::parse_docker_stats(
        "aaaaaaaaaaaa\t12.5%\t42.1MiB / 1GiB\t4.1%\ninvalid\tNaN\tbad\t101%",
        now,
    );
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].container_id, "aaaaaaaaaaaa");
    assert_eq!(samples[0].cpu_basis_points, Some(1250));
    assert_eq!(samples[0].memory_basis_points, Some(410));
    assert_eq!(samples[0].memory_used_bytes, Some(44_145_050));
    assert_eq!(samples[0].memory_limit_bytes, Some(1_073_741_824));
}

#[test]
fn remote_manifest_digest_matches_the_running_platform() {
    let manifest = r#"[{"Platform":{"os":"linux","architecture":"amd64"},"SchemaV2Manifest":{"config":{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}},{"Platform":{"os":"linux","architecture":"arm64"},"SchemaV2Manifest":{"config":{"digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}]"#;
    assert_eq!(
        parse_remote_image_config_digest(manifest, "linux/arm64").as_deref(),
        Some("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    assert_eq!(
        parse_remote_image_config_digest(manifest, "windows/amd64"),
        None
    );
}

#[tokio::test]
async fn docker_lifecycle_client_rejects_non_hex_ids_before_network_access() {
    let config = AgentClientConfig::new("https://agent.example", "token").unwrap();
    let client = AgentClient::new(config).unwrap();
    let error = client
        .docker_lifecycle(&DockerLifecycleRequest {
            container_id: "../../etc/passwd".to_owned(),
            action: DockerLifecycleAction::Restart,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::InvalidRequest(_)));
}

#[test]
fn docker_inspect_parser_extracts_safe_optional_metadata() {
    let metadata = parse_docker_inspect_metadata(
        r#"[{"Id":"aaaaaaaaaaaa","Image":"sha256:abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd","Created":"2026-01-01T00:00:00Z","RestartCount":3,"State":{"StartedAt":"2026-01-02T00:00:00Z","OOMKilled":true,"Health":{"Status":"unhealthy"}}}]"#,
    );
    let item = metadata.get("aaaaaaaaaaaa").unwrap();
    assert_eq!(item.created_at.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!(item.started_at.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert_eq!(
        item.image_id.as_deref(),
        Some("sha256:abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd")
    );
    assert_eq!(item.restart_count, Some(3));
    assert_eq!(item.health.as_deref(), Some("unhealthy"));
    assert_eq!(item.oom_killed, Some(true));

    let no_health = parse_docker_inspect_metadata(
        r#"[{"Id":"bbbbbbbbbbbb","Image":"bad value","RestartCount":-1,"State":{"Health":{"Status":"unknown"}}}]"#,
    );
    let item = no_health.get("bbbbbbbbbbbb").unwrap();
    assert_eq!(item.image_id, None);
    assert_eq!(item.restart_count, None);
    assert_eq!(item.health, None);
    assert_eq!(item.oom_killed, None);

    let old_agent_payload = serde_json::json!({
        "id": "aaaaaaaaaaaa",
        "name": "web",
        "image": "nginx:latest",
        "state": "running",
        "status": "Up",
        "ports": [],
        "started_at": null,
        "labels": []
    });
    let old_agent_info: super::DockerContainerInfo =
        serde_json::from_value(old_agent_payload).unwrap();
    assert_eq!(old_agent_info.health, None);
    assert_eq!(old_agent_info.restart_count, None);
    assert_eq!(old_agent_info.oom_killed, None);
}

#[test]
fn docker_failure_reason_distinguishes_socket_permissions_from_missing_cli() {
    assert_eq!(
        docker_failure_reason(
            b"permission denied while trying to connect to the docker API",
            None
        ),
        "docker_permission_denied"
    );
    assert_eq!(
        docker_failure_reason(&[], Some(std::io::ErrorKind::NotFound)),
        "docker_cli_unavailable"
    );
    assert_eq!(
        docker_failure_reason(b"daemon stopped", None),
        "docker_unavailable"
    );
}

#[test]
fn package_inventory_parsers_normalize_linux_and_windows_fixtures() {
    let linux = parse_dpkg_packages("curl\t8.5.0-2\tamd64\ninvalid");
    assert_eq!(linux.len(), 1);
    assert_eq!(linux[0].source.as_deref(), Some("dpkg"));
    let windows = parse_windows_packages(
        "Name                  Id                    Version  Available  Source\n--------------------------------------------------------------------------------\n7-Zip 24.09 (x64)      7zip.7zip             24.09    25.01      winget\n",
    )
    .unwrap();
    assert_eq!(windows[0].name, "7zip.7zip");
    assert_eq!(windows[0].candidate_version.as_deref(), Some("25.01"));
    assert_eq!(windows[0].source.as_deref(), Some("winget"));
}

#[test]
fn windows_package_parser_accepts_empty_and_rejects_bad_or_excessive_output() {
    assert!(
        parse_windows_packages("Name  Id  Version  Source\n-------------------------\n")
            .unwrap()
            .is_empty()
    );
    assert!(parse_windows_packages("").is_err());
    assert!(parse_windows_packages("null").is_err());
    assert!(parse_windows_packages("winget failed to start").is_err());
    let excessive = format!(
        "Name  Id  Version  Source\n-------------------------\n{}",
        (0..=50_000)
            .map(|_| "Package  Vendor.Package  1.0  winget")
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(parse_windows_packages(&excessive).is_err());
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
async fn local_windows_workflow_commands_are_cached_and_reject_unsafe_packages() {
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "windows-workflow-agent".to_owned(),
            platform: AgentPlatform::Windows,
            hostname: "windows-host".to_owned(),
            version: "0.5.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "agent-token",
    );
    let health_command = AgentWorkflowCommand {
        job_id: uuid::Uuid::new_v4(),
        action: AgentAction::Health,
        packages: Vec::new(),
    };

    let first = execute_workflow_command(&state, &health_command).await;
    let repeated = execute_workflow_command(&state, &health_command).await;
    assert!(first.response.success);
    assert_eq!(first.response.stdout, "healthy");
    assert_eq!(first.response.request_id, repeated.response.request_id);

    let unsafe_command = AgentWorkflowCommand {
        job_id: uuid::Uuid::new_v4(),
        action: AgentAction::Apply,
        packages: vec!["--accept-all".to_owned()],
    };
    let rejected = execute_workflow_command(&state, &unsafe_command).await;
    assert!(!rejected.response.success);
    assert_eq!(rejected.response.exit_code, 2);
    let metrics = state.metrics_snapshot().await;
    assert_eq!(metrics.commands_total, 2);
    assert_eq!(metrics.commands_failed, 1);
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
        StatusCode::OK
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
            | StatusCode::BAD_GATEWAY
    ));
}

#[tokio::test]
async fn docker_mutation_and_image_update_routes_guard_auth_platform_and_ids() {
    let windows = agent_router(LocalAgentState::new(
        AgentInfo {
            agent_id: "windows-agent".to_owned(),
            platform: AgentPlatform::Windows,
            hostname: "windows-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    ));
    let linux = agent_router(LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    ));

    for (path, payload) in [
        (
            "/docker/containers/action",
            serde_json::json!({"container_id":"aaaaaaaaaaaa", "action":"restart"}),
        ),
        (
            "/docker/containers/image-update-check",
            serde_json::json!({"container_id":"aaaaaaaaaaaa"}),
        ),
        (
            "/docker/containers/image-update-apply",
            serde_json::json!({"container_id":"aaaaaaaaaaaa", "expected_remote_image_id":format!("sha256:{}", "a".repeat(64))}),
        ),
    ] {
        let unauthorized = linux
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let unsupported = windows
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", "Bearer docker-token")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
    }

    for (path, payload) in [
        (
            "/docker/containers/action",
            serde_json::json!({"container_id":"not-an-id", "action":"restart"}),
        ),
        (
            "/docker/containers/image-update-check",
            serde_json::json!({"container_id":"not-an-id"}),
        ),
        (
            "/docker/containers/image-update-apply",
            serde_json::json!({"container_id":"not-an-id", "expected_remote_image_id":"invalid"}),
        ),
    ] {
        let invalid = linux
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", "Bearer docker-token")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn compose_image_update_recreates_only_the_unchanged_service_without_dependencies() {
    let project_dir = std::env::temp_dir().join(format!("lxcup-compose-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&project_dir).unwrap();
    let config_file = project_dir.join("compose.yaml");
    std::fs::write(&config_file, "services: {web: {image: nginx:stable}}\n").unwrap();
    let project_dir = project_dir.canonicalize().unwrap();
    let config_file = config_file.canonicalize().unwrap();
    let container_id = "a".repeat(64);
    let previous_image_id = format!("sha256:{}", "a".repeat(64));
    let remote_image_id = format!("sha256:{}", "b".repeat(64));
    let labels = serde_json::json!({
        "com.docker.compose.project": "shop",
        "com.docker.compose.service": "web",
        "com.docker.compose.project.working_dir": project_dir.to_string_lossy(),
        "com.docker.compose.project.config_files": config_file.to_string_lossy(),
        "com.docker.compose.config-hash": "unchanged-hash",
        "com.docker.compose.container-number": "1"
    });
    let inspect = format!("{container_id}\tnginx:stable\t{previous_image_id}\t{labels}\n");
    let manifest = format!(
        r#"[{{"Platform":{{"os":"linux","architecture":"amd64"}},"SchemaV2Manifest":{{"config":{{"digest":"{remote_image_id}"}}}}}}]"#
    );
    let runner = Arc::new(ScriptedDockerRunner::default());
    runner.outputs.lock().unwrap().extend([
        docker_result(true, inspect.clone()),
        docker_result(true, "unchanged-hash\n"),
        docker_result(true, "linux/amd64\n"),
        docker_result(true, manifest.clone()),
        docker_result(true, ""),
        docker_result(true, format!("{remote_image_id}\n")),
        docker_result(true, format!("{container_id}\n")),
        docker_result(true, ""),
        docker_result(true, format!("{container_id}\n")),
        docker_result(true, "running\thealthy\n"),
    ]);
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    )
    .with_docker_command_runner(runner.clone());
    let response = agent_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/image-update-apply")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": container_id,
                        "expected_remote_image_id": remote_image_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let applied: DockerImageUpdateApplyResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(applied.compose_project, "shop");
    assert_eq!(applied.compose_service, "web");
    assert_eq!(applied.image_id, remote_image_id);
    assert_eq!(applied.service_state, "running");
    assert_eq!(applied.health_status.as_deref(), Some("healthy"));

    {
        let commands = runner.commands.lock().unwrap();
        assert_eq!(commands.len(), 10);
        assert!(
            commands[4]
                .0
                .ends_with(&["pull".to_owned(), "web".to_owned()])
        );
        assert!(commands[6].0.ends_with(&[
            "ps".to_owned(),
            "--all".to_owned(),
            "--quiet".to_owned(),
            "web".to_owned()
        ]));
        assert!(commands[7].0.ends_with(&[
            "up".to_owned(),
            "--detach".to_owned(),
            "--no-deps".to_owned(),
            "--force-recreate".to_owned(),
            "--wait".to_owned(),
            "--wait-timeout".to_owned(),
            "120".to_owned(),
            "web".to_owned()
        ]));
        assert_eq!(commands[1].1.as_deref(), Some(project_dir.as_path()));
        assert_eq!(commands[4].1.as_deref(), Some(project_dir.as_path()));
    }

    runner.outputs.lock().unwrap().extend([
        docker_result(true, inspect),
        docker_result(true, "unchanged-hash\n"),
        docker_result(true, "linux/amd64\n"),
        docker_result(true, manifest),
        docker_result(true, ""),
        docker_result(true, format!("{remote_image_id}\n")),
        docker_result(true, format!("{container_id}\n")),
        docker_result(true, ""),
        docker_result(true, format!("{container_id}\n")),
        docker_result(true, "running\tunhealthy\n"),
    ]);
    let response = agent_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/image-update-apply")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": container_id,
                        "expected_remote_image_id": remote_image_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
    let failure: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(failure["error"], "updated_service_unhealthy");
    std::fs::remove_dir_all(project_dir).unwrap();
}

#[tokio::test]
async fn compose_image_update_stops_when_the_saved_configuration_hash_changed() {
    let project_dir = std::env::temp_dir().join(format!("lxcup-compose-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&project_dir).unwrap();
    let config_file = project_dir.join("compose.yaml");
    std::fs::write(&config_file, "services: {web: {image: nginx:stable}}\n").unwrap();
    let project_dir = project_dir.canonicalize().unwrap();
    let config_file = config_file.canonicalize().unwrap();
    let container_id = "a".repeat(64);
    let labels = serde_json::json!({
        "com.docker.compose.project": "shop",
        "com.docker.compose.service": "web",
        "com.docker.compose.project.working_dir": project_dir.to_string_lossy(),
        "com.docker.compose.project.config_files": config_file.to_string_lossy(),
        "com.docker.compose.config-hash": "old-hash",
        "com.docker.compose.container-number": "1"
    });
    let runner = Arc::new(ScriptedDockerRunner::default());
    runner.outputs.lock().unwrap().extend([
        docker_result(
            true,
            format!(
                "{container_id}\tnginx:stable\tsha256:{}\t{labels}\n",
                "a".repeat(64)
            ),
        ),
        docker_result(true, "new-hash\n"),
    ]);
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    )
    .with_docker_command_runner(runner.clone());
    let response = agent_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/image-update-apply")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "container_id": container_id,
                        "expected_remote_image_id": format!("sha256:{}", "b".repeat(64))
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let commands = runner.commands.lock().unwrap();
    assert_eq!(commands.len(), 2);
    drop(commands);
    std::fs::remove_dir_all(project_dir).unwrap();
}

#[tokio::test]
async fn docker_discovery_and_image_check_report_metadata_and_registry_state() {
    let container_id = "c".repeat(64);
    let previous_image_id = format!("sha256:{}", "d".repeat(64));
    let remote_image_id = format!("sha256:{}", "e".repeat(64));
    let runner = Arc::new(ScriptedDockerRunner::default());
    runner.outputs.lock().unwrap().extend([
        docker_result(
            true,
            format!(
                "{container_id}\tweb\tnginx:stable\trunning\tUp 1 minute\t80/tcp\tcom.docker.compose.project=shop,com.docker.compose.service=web\n"
            ),
        ),
        docker_result(
            true,
            format!(
                r#"[{{"Id":"{container_id}","Image":"{previous_image_id}","Created":"2026-09-01T00:00:00Z","RestartCount":2,"State":{{"StartedAt":"2026-09-02T00:00:00Z","OOMKilled":false,"Health":{{"Status":"healthy"}}}}}}]"#
            ),
        ),
        docker_result(
            true,
            format!("nginx:stable\t{previous_image_id}\n"),
        ),
        docker_result(true, "linux/amd64\n"),
        docker_result(
            true,
            format!(
                r#"[{{"Platform":{{"os":"linux","architecture":"amd64"}},"SchemaV2Manifest":{{"config":{{"digest":"{remote_image_id}"}}}}}}]"#
            ),
        ),
    ]);
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    )
    .with_docker_command_runner(runner.clone());
    let api = agent_router(state);

    let discovery = api
        .clone()
        .oneshot(
            Request::builder()
                .uri("/docker/containers")
                .header("authorization", "Bearer docker-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(discovery.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(discovery.into_body(), usize::MAX)
        .await
        .unwrap();
    let inventory: DockerDiscovery = serde_json::from_slice(&body).unwrap();
    assert!(inventory.available);
    assert_eq!(inventory.containers[0].name, "web");
    assert_eq!(
        inventory.containers[0].image_id.as_deref(),
        Some(previous_image_id.as_str())
    );
    assert_eq!(inventory.containers[0].restart_count, Some(2));
    assert_eq!(inventory.containers[0].health.as_deref(), Some("healthy"));

    let checked = api
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/image-update-check")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"container_id":container_id}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(checked.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(checked.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: DockerImageUpdateResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(result.status, DockerImageUpdateStatus::UpdateAvailable);
    assert_eq!(
        result.current_image_id.as_deref(),
        Some(previous_image_id.as_str())
    );
    assert_eq!(
        result.remote_image_id.as_deref(),
        Some(remote_image_id.as_str())
    );
    assert_eq!(runner.commands.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn docker_lifecycle_returns_success_and_sanitized_command_errors() {
    let runner = Arc::new(ScriptedDockerRunner::default());
    runner.outputs.lock().unwrap().extend([
        docker_result(true, ""),
        crate::docker::DockerCommandOutput {
            success: false,
            stdout: Vec::new(),
            stderr: b"permission denied while connecting to docker.sock".to_vec(),
        },
    ]);
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    )
    .with_docker_command_runner(runner.clone());
    let api = agent_router(state);
    let container_id = "f".repeat(64);

    let started = api
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/action")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"container_id":container_id,"action":"start"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), axum::http::StatusCode::OK);

    let denied = api
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/action")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"container_id":container_id,"action":"stop"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["error"], "docker_permission_denied");
    assert_eq!(runner.commands.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn production_docker_runner_reports_version_without_mutating_the_engine() {
    let args = vec!["--version".to_owned()];
    let attempt = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::docker::ProcessDockerCommandRunner.output(&args, None),
    )
    .await;
    assert!(
        attempt.is_ok(),
        "Docker version probe should return promptly"
    );
    if let Err(error) = attempt.unwrap() {
        assert!(matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        ));
    }
}

#[tokio::test]
async fn docker_discovery_exposes_permission_errors_and_digest_pinned_images() {
    let pinned_digest = format!("sha256:{}", "a".repeat(64));
    let runner = Arc::new(ScriptedDockerRunner::default());
    runner.outputs.lock().unwrap().extend([
        crate::docker::DockerCommandOutput {
            success: false,
            stdout: Vec::new(),
            stderr: b"permission denied while trying to connect to the Docker daemon".to_vec(),
        },
        docker_result(true, format!("nginx@{pinned_digest}\t{pinned_digest}\n")),
    ]);
    let state = LocalAgentState::new(
        AgentInfo {
            agent_id: "linux-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "linux-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        "docker-token",
    )
    .with_docker_command_runner(runner);
    let api = agent_router(state);

    let unavailable = api
        .clone()
        .oneshot(
            Request::builder()
                .uri("/docker/containers")
                .header("authorization", "Bearer docker-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(unavailable.into_body(), usize::MAX)
        .await
        .unwrap();
    let discovery: DockerDiscovery = serde_json::from_slice(&body).unwrap();
    assert!(!discovery.available);
    assert_eq!(
        discovery.reason.as_deref(),
        Some("docker_permission_denied")
    );

    let checked = api
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/docker/containers/image-update-check")
                .header("authorization", "Bearer docker-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"container_id":"aaaaaaaaaaaa"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(checked.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: DockerImageUpdateResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(result.status, DockerImageUpdateStatus::Pinned);
    assert_eq!(result.reason.as_deref(), Some("digest_pinned"));
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
