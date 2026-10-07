use super::{
    AgentAction, AgentCommandRequest, AgentCommandResponse, AgentHealth, AgentPlatform,
    LocalAgentState, PackageInventoryError, collect_package_inventory, docker, normalize_apt_list,
    safe_detail, safe_package, safe_winget_id,
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
    match collect_package_inventory(state.info.platform).await {
        Ok(inventory) => Json(inventory).into_response(),
        Err(error) => package_inventory_error_response(error),
    }
}

fn package_inventory_error_response(error: PackageInventoryError) -> axum::response::Response {
    let (status, code) = match error {
        PackageInventoryError::ManagerUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "package_manager_unavailable",
        ),
        PackageInventoryError::Timeout => {
            (StatusCode::GATEWAY_TIMEOUT, "package_inventory_timeout")
        }
        PackageInventoryError::TooLarge => {
            (StatusCode::PAYLOAD_TOO_LARGE, "package_inventory_too_large")
        }
        PackageInventoryError::InvalidOutput => {
            (StatusCode::BAD_GATEWAY, "package_inventory_invalid")
        }
    };
    (status, Json(serde_json::json!({"error": code}))).into_response()
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
    if state.info.platform == AgentPlatform::Windows && request.action == AgentAction::Apply {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "windows_apply_requires_approved_workflow"
            })),
        )
            .into_response();
    }
    if request.action == AgentAction::UpdateAgent {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error":"agent_update_requires_approved_workflow"})),
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

pub async fn execute_workflow_command(
    state: &LocalAgentState,
    command: &super::AgentWorkflowCommand,
) -> super::AgentWorkflowResult {
    let idempotency_key = command.job_id.to_string();
    let mut results = state.results.lock().await;
    if let Some(response) = results.get(&idempotency_key).cloned() {
        return super::AgentWorkflowResult { response };
    }
    let started = std::time::Instant::now();
    let (exit_code, stdout, stderr) = if command.action == AgentAction::UpdateAgent {
        (
            2,
            String::new(),
            "agent update must run through the Windows service updater".to_owned(),
        )
    } else if command.agent_version.is_some() && command.action == AgentAction::Health {
        let version = command.agent_version.as_deref().unwrap_or_default();
        if !safe_agent_version(version) {
            (2, String::new(), "invalid agent update version".to_owned())
        } else if version == env!("CARGO_PKG_VERSION") {
            (
                0,
                format!("Installed agent version {version} is confirmed."),
                String::new(),
            )
        } else {
            (
                1,
                format!(
                    "Installed agent version is {}; expected {version}.",
                    env!("CARGO_PKG_VERSION")
                ),
                String::new(),
            )
        }
    } else if state.info.platform == AgentPlatform::Windows
        && command.action == AgentAction::Scan
        && command.packages.is_empty()
    {
        (
            0,
            "Package inventory refresh is handled by the agent inventory collector.".to_owned(),
            String::new(),
        )
    } else if command
        .packages
        .iter()
        .any(|package| !safe_package(package))
    {
        (2, String::new(), "invalid package selection".to_owned())
    } else {
        run_local_command(state.info.platform, command.action, &command.packages).await
    };
    let success = exit_code == 0;
    let now = Utc::now();
    let mut metrics = state.metrics.lock().await;
    metrics.commands_total = metrics.commands_total.saturating_add(1);
    if !success {
        metrics.commands_failed = metrics.commands_failed.saturating_add(1);
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
    if results.len() >= 512 {
        results.clear();
    }
    results.insert(idempotency_key, response.clone());
    super::AgentWorkflowResult { response }
}

fn safe_agent_version(version: &str) -> bool {
    let (release, prerelease) = version.split_once('-').unwrap_or((version, ""));
    let mut components = release.split('.');
    (0..3).all(|_| {
        components
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && components.next().is_none()
        && version.len() <= 50
        && (prerelease.is_empty()
            || prerelease
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
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
    if platform == AgentPlatform::Windows {
        return run_windows_winget_command(action, packages).await;
    }
    let mut command = {
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
            AgentAction::UpdateAgent => {
                return (
                    2,
                    String::new(),
                    "agent update requires an approved workflow".to_owned(),
                );
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

async fn run_windows_winget_command(
    action: AgentAction,
    packages: &[String],
) -> (i32, String, String) {
    if action == AgentAction::Health {
        return (0, "healthy".to_owned(), String::new());
    }
    if action == AgentAction::UpdateAgent {
        return (
            2,
            String::new(),
            "agent update requires an approved workflow".to_owned(),
        );
    }
    let invocation = match windows_winget_invocation(action, packages) {
        Ok(invocation) => invocation,
        Err(message) => return (2, String::new(), message.to_owned()),
    };
    let result = tokio::time::timeout(Duration::from_secs(900), async move {
        match Command::new(crate::windows_powershell_program())
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                invocation.script,
            ])
            .env("LXCUP_WINGET_PACKAGE_IDS", invocation.package_ids)
            .output()
            .await
        {
            Ok(output) => (
                output.status.code().unwrap_or(1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ),
            Err(error) => (1, String::new(), error.to_string()),
        }
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => (124, String::new(), "agent command timed out".to_owned()),
    }
}

struct WindowsWingetInvocation {
    script: &'static str,
    package_ids: String,
}

fn windows_winget_invocation(
    action: AgentAction,
    packages: &[String],
) -> Result<WindowsWingetInvocation, &'static str> {
    let script = match action {
        AgentAction::Health => return Err("health action does not use winget"),
        AgentAction::UpdateAgent => return Err("agent update does not use winget"),
        AgentAction::Scan => {
            r#"$ErrorActionPreference = 'Stop'
Import-Module Microsoft.WinGet.Client -ErrorAction Stop
$packages = @(Get-WinGetPackage -ErrorAction Stop | Where-Object { $_.IsUpdateAvailable } | ForEach-Object {
    $candidate = $null
    if (@($_.AvailableVersions).Count -gt 0) { $candidate = [string]@($_.AvailableVersions)[0] }
    [ordered]@{ id = [string]$_.Id; name = [string]$_.Name; version = [string]$_.Version; candidate_version = $candidate; source = [string]$_.Source }
})
ConvertTo-Json -InputObject $packages -Compress -Depth 4"#
        }
        AgentAction::Apply => {
            if packages.is_empty() {
                return Err("apply requires at least one winget package ID");
            }
            if packages.iter().any(|package| !safe_winget_id(package)) {
                return Err("apply contains an invalid winget package ID");
            }
            r#"$ErrorActionPreference = 'Stop'
Import-Module Microsoft.WinGet.Client -ErrorAction Stop
$ids = @(ConvertFrom-Json -InputObject $env:LXCUP_WINGET_PACKAGE_IDS -ErrorAction Stop)
foreach ($id in $ids) {
    Update-WinGetPackage -Id ([string]$id) -MatchOption EqualsCaseInsensitive -Scope System -Confirm:$false -ErrorAction Stop | Out-Null
    "Updated $id"
}"#
        }
    };
    let package_ids = serde_json::to_string(packages).map_err(|_| "invalid package selection")?;
    Ok(WindowsWingetInvocation {
        script,
        package_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        execute_workflow_command, run_local_command, safe_winget_id, windows_winget_invocation,
    };
    use crate::{AgentAction, AgentPlatform, AgentWorkflowCommand};
    use tower::ServiceExt;

    #[tokio::test]
    async fn linux_apply_requires_at_least_one_package() {
        let (exit_code, stdout, stderr) =
            run_local_command(AgentPlatform::Linux, AgentAction::Apply, &[]).await;

        assert_eq!(exit_code, 2);
        assert!(stdout.is_empty());
        assert_eq!(stderr, "apply requires at least one package");
    }

    #[tokio::test]
    async fn windows_direct_apply_is_rejected_before_running_a_command() {
        let state = crate::LocalAgentState::new(
            crate::AgentInfo {
                agent_id: "windows-agent".to_owned(),
                platform: AgentPlatform::Windows,
                hostname: "windows-host".to_owned(),
                version: "0.4.0".to_owned(),
                protocol_version: crate::PROTOCOL_VERSION.to_owned(),
            },
            "windows-token",
        );
        let response = crate::agent_router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/command")
                    .header("authorization", "Bearer windows-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "action": "apply",
                            "packages": ["security update"],
                            "idempotency_key": "windows-direct-apply"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "windows_apply_requires_approved_workflow"
        );
    }

    #[tokio::test]
    async fn windows_inventory_workflow_skips_the_update_scan_command() {
        let state = crate::LocalAgentState::new(
            crate::AgentInfo {
                agent_id: "windows-agent".to_owned(),
                platform: AgentPlatform::Windows,
                hostname: "windows-host".to_owned(),
                version: "0.6.0".to_owned(),
                protocol_version: crate::PROTOCOL_VERSION.to_owned(),
            },
            "windows-token",
        );
        let result = execute_workflow_command(
            &state,
            &AgentWorkflowCommand {
                job_id: uuid::Uuid::new_v4(),
                action: AgentAction::Scan,
                packages: Vec::new(),
                agent_version: None,
            },
        )
        .await;

        assert!(result.response.success);
        assert_eq!(result.response.exit_code, 0);
        assert_eq!(
            result.response.stdout,
            "Package inventory refresh is handled by the agent inventory collector."
        );
    }

    #[test]
    fn windows_update_scan_uses_the_system_supported_winget_client_module() {
        let invocation = windows_winget_invocation(AgentAction::Scan, &[]).unwrap();

        assert!(
            invocation
                .script
                .contains("Import-Module Microsoft.WinGet.Client")
        );
        assert!(invocation.script.contains("Get-WinGetPackage"));
        assert!(invocation.script.contains("$_.IsUpdateAvailable"));
        assert!(!invocation.script.contains("winget.exe"));
        assert!(!invocation.script.contains("winget list"));
        assert_eq!(invocation.package_ids, "[]");
    }

    #[test]
    fn windows_update_apply_is_limited_to_exact_validated_ids() {
        let invocation = windows_winget_invocation(
            AgentAction::Apply,
            &["Microsoft.PowerToys".to_owned(), "7zip.7zip".to_owned()],
        )
        .unwrap();

        assert!(invocation.script.contains("Update-WinGetPackage"));
        assert!(
            invocation
                .script
                .contains("-MatchOption EqualsCaseInsensitive")
        );
        assert!(invocation.script.contains("-Confirm:$false"));
        assert!(invocation.script.contains("-Scope System"));
        assert_eq!(
            invocation.package_ids,
            r#"["Microsoft.PowerToys","7zip.7zip"]"#
        );
    }

    #[test]
    fn windows_update_apply_rejects_empty_or_argument_shaped_ids() {
        assert!(windows_winget_invocation(AgentAction::Apply, &[]).is_err());
        for package in ["--all", "Microsoft.PowerToys; whoami", "Publisher..App"] {
            assert!(!safe_winget_id(package), "unexpectedly accepted {package}");
            assert!(windows_winget_invocation(AgentAction::Apply, &[package.to_owned()]).is_err());
        }
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
