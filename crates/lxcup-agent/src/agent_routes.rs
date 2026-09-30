use super::{
    AgentAction, AgentCommandRequest, AgentCommandResponse, AgentHealth, AgentPackageInventory,
    AgentPlatform, LocalAgentState, docker, normalize_apt_list, parse_dpkg_packages,
    parse_windows_packages, safe_detail, safe_package,
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

pub fn agent_router(state: LocalAgentState) -> Router {
    Router::new()
        .route("/health", get(agent_health))
        .route("/metrics", get(agent_metrics))
        .route("/docker/containers", get(docker::agent_docker_containers))
        .route(
            "/docker/containers/action",
            post(docker::agent_docker_lifecycle),
        )
        .route(
            "/docker/containers/image-update-check",
            post(docker::agent_docker_image_update_check),
        )
        .route(
            "/docker/containers/image-update-apply",
            post(docker::agent_docker_image_update_apply),
        )
        .route("/packages", get(agent_package_inventory))
        .route("/command", post(agent_command))
        .with_state(state)
}

async fn agent_package_inventory(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    let mut command = if state.info.platform == AgentPlatform::Windows {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "Get-Package | Select-Object -Property Name,Version,ProviderName | ConvertTo-Json -Compress"]);
        command
    } else {
        let mut command = Command::new("dpkg-query");
        command.args([
            "-W",
            "-f=${binary:Package}\\t${Version}\\t${Architecture}\\n",
        ]);
        command
    };
    let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output.stdout,
        Ok(Ok(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"package_manager_unavailable"})),
            )
                .into_response();
        }
        Ok(Err(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"package_manager_unavailable"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(serde_json::json!({"error":"package_inventory_timeout"})),
            )
                .into_response();
        }
    };
    let packages = if state.info.platform == AgentPlatform::Windows {
        parse_windows_packages(&String::from_utf8_lossy(&output))
    } else {
        parse_dpkg_packages(&String::from_utf8_lossy(&output))
    };
    if packages.len() > 50_000 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error":"package_inventory_too_large"})),
        )
            .into_response();
    }
    Json(AgentPackageInventory {
        collected_at: Utc::now(),
        packages,
    })
    .into_response()
}

async fn agent_health(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    Json(AgentHealth {
        healthy: true,
        info: state.info.clone(),
        metrics: state.metrics.lock().await.clone(),
    })
    .into_response()
}

async fn agent_metrics(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    Json(state.metrics.lock().await.clone()).into_response()
}

async fn agent_command(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<AgentCommandRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if request.idempotency_key.trim().is_empty()
        || request
            .packages
            .iter()
            .any(|package| !safe_package(package))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"invalid_request"})),
        )
            .into_response();
    }
    if let Some(cached) = state
        .results
        .lock()
        .await
        .get(&request.idempotency_key)
        .cloned()
    {
        return Json(cached).into_response();
    }
    tracing::info!(agent_id = %state.info.agent_id, action = ?request.action, idempotency_key = %request.idempotency_key, "agent command requested");
    let started = std::time::Instant::now();
    let (exit_code, stdout, stderr) =
        run_local_command(state.info.platform, request.action, &request.packages).await;
    let success = exit_code == 0;
    let now = Utc::now();
    let mut metrics = state.metrics.lock().await;
    metrics.commands_total += 1;
    if !success {
        metrics.commands_failed += 1;
    }
    metrics.last_command_at = Some(now);
    metrics.collected_at = now;
    drop(metrics);
    let response = AgentCommandResponse {
        request_id: Uuid::new_v4(),
        success,
        exit_code,
        stdout: safe_detail(&stdout),
        stderr: safe_detail(&stderr),
        reboot_required: false,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    let mut results = state.results.lock().await;
    if results.len() >= 512 {
        results.clear();
    }
    results.insert(request.idempotency_key, response.clone());
    Json(response).into_response()
}

pub(crate) fn authorized(state: &LocalAgentState, headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == state.token.as_ref())
}

async fn run_local_command(
    platform: AgentPlatform,
    action: AgentAction,
    packages: &[String],
) -> (i32, String, String) {
    let mut command = if platform == AgentPlatform::Windows {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command"]);
        let script = match action {
            AgentAction::Health => "Write-Output 'healthy'",
            AgentAction::Scan => {
                "if (Get-Command Get-WindowsUpdate -ErrorAction SilentlyContinue) { Get-WindowsUpdate -MicrosoftUpdate -IgnoreReboot | ConvertTo-Json -Compress } else { Write-Error 'PSWindowsUpdate is not installed'; exit 2 }"
            }
            AgentAction::Apply => {
                "if (Get-Command Install-WindowsUpdate -ErrorAction SilentlyContinue) { Install-WindowsUpdate -AcceptAll -IgnoreReboot } else { Write-Error 'PSWindowsUpdate is not installed'; exit 2 }"
            }
        };
        command.arg(script);
        command
    } else {
        let mut command = Command::new("apt");
        match action {
            AgentAction::Health => {
                command.arg("--version");
            }
            AgentAction::Scan => {
                command.args(["list", "--upgradable"]);
            }
            AgentAction::Apply => {
                if packages.is_empty() {
                    return (
                        2,
                        String::new(),
                        "apply requires at least one package".to_owned(),
                    );
                }
                command.args(["--yes", "--only-upgrade", "install"]);
                command.args(packages);
            }
        }
        command
    };
    match tokio::time::timeout(Duration::from_secs(900), command.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stdout = if platform == AgentPlatform::Linux && action == AgentAction::Scan {
                normalize_apt_list(&stdout)
            } else {
                stdout
            };
            (
                output.status.code().unwrap_or(1),
                stdout,
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        }
        Ok(Err(error)) => (1, String::new(), error.to_string()),
        Err(_) => (124, String::new(), "agent command timed out".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::run_local_command;
    use crate::{AgentAction, AgentPlatform};

    #[tokio::test]
    async fn linux_apply_requires_at_least_one_package() {
        let (exit_code, stdout, stderr) =
            run_local_command(AgentPlatform::Linux, AgentAction::Apply, &[]).await;

        assert_eq!(exit_code, 2);
        assert!(stdout.is_empty());
        assert_eq!(stderr, "apply requires at least one package");
    }

    #[tokio::test]
    async fn local_health_actions_use_the_platform_command_path() {
        for platform in [AgentPlatform::Linux, AgentPlatform::Windows] {
            let (exit_code, _, stderr) =
                run_local_command(platform, AgentAction::Health, &[]).await;
            assert_ne!(exit_code, 124, "health action should not time out");
            if exit_code != 0 {
                assert!(!stderr.is_empty());
            }
        }
    }
}
