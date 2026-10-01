use axum::{
    Json,
    extract::{Json as JsonBody, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde_json::Value;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};
use tokio::process::Command;

mod compose_update_status;

use super::parsers::{
    docker_failure_reason, parse_docker_containers, parse_docker_inspect_metadata,
    parse_remote_image_config_digest,
};
use super::{
    AgentPlatform, DockerDiscovery, DockerImageUpdateApplyRequest, DockerImageUpdateApplyResult,
    DockerImageUpdateRequest, DockerImageUpdateResult, DockerImageUpdateStatus,
    DockerLifecycleAction, DockerLifecycleRequest, DockerLifecycleResult, LocalAgentState,
    authorized, is_safe_docker_container_id,
};

pub(crate) trait DockerCommandRunner: Send + Sync {
    fn output<'a>(
        &'a self,
        args: &'a [String],
        working_directory: Option<&'a Path>,
    ) -> Pin<Box<dyn Future<Output = io::Result<DockerCommandOutput>> + Send + 'a>>;
}

#[derive(Clone, Debug)]
pub(crate) struct DockerCommandOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) struct ProcessDockerCommandRunner;

impl DockerCommandRunner for ProcessDockerCommandRunner {
    fn output<'a>(
        &'a self,
        args: &'a [String],
        working_directory: Option<&'a Path>,
    ) -> Pin<Box<dyn Future<Output = io::Result<DockerCommandOutput>> + Send + 'a>> {
        Box::pin(async move {
            let mut command = Command::new("docker");
            command.args(args);
            if let Some(directory) = working_directory {
                command.current_dir(directory);
            }
            command.output().await.map(|output| DockerCommandOutput {
                success: output.status.success(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }
}

impl LocalAgentState {
    async fn docker_output(
        &self,
        args: &[String],
        working_directory: Option<&Path>,
    ) -> io::Result<DockerCommandOutput> {
        self.docker_command_runner
            .output(args, working_directory)
            .await
    }
}

pub(super) async fn agent_docker_containers(
    State(state): State<LocalAgentState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if state.info.platform == AgentPlatform::Windows {
        return Json(DockerDiscovery {
            available: false,
            reason: Some("unsupported_platform".to_owned()),
            collected_at: Utc::now(),
            containers: Vec::new(),
        })
        .into_response();
    }

    let output = match state
        .docker_output(
            &[
                "ps".to_owned(),
                "--all".to_owned(),
                "--no-trunc".to_owned(),
                "--format".to_owned(),
                "{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.State}}\\t{{.Status}}\\t{{.Ports}}\\t{{.Labels}}".to_owned(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => output.stdout,
        Ok(output) => {
            let reason = docker_failure_reason(&output.stderr, None);
            return Json(DockerDiscovery {
                available: false,
                reason: Some(reason.to_owned()),
                collected_at: Utc::now(),
                containers: Vec::new(),
            })
            .into_response();
        }
        Err(error) => {
            let reason = docker_failure_reason(&[], Some(error.kind()));
            return Json(DockerDiscovery {
                available: false,
                reason: Some(reason.to_owned()),
                collected_at: Utc::now(),
                containers: Vec::new(),
            })
            .into_response();
        }
    };

    let mut containers = parse_docker_containers(&String::from_utf8_lossy(&output));
    if !containers.is_empty() {
        let mut args = vec!["inspect".to_owned()];
        args.extend(containers.iter().map(|container| container.id.clone()));
        let inspect = state.docker_output(&args, None).await;
        let inspect = match inspect {
            Ok(output) if output.success => output.stdout,
            Ok(output) => {
                let reason = docker_failure_reason(&output.stderr, None);
                return Json(DockerDiscovery {
                    available: false,
                    reason: Some(reason.to_owned()),
                    collected_at: Utc::now(),
                    containers: Vec::new(),
                })
                .into_response();
            }
            Err(error) => {
                let reason = docker_failure_reason(&[], Some(error.kind()));
                return Json(DockerDiscovery {
                    available: false,
                    reason: Some(reason.to_owned()),
                    collected_at: Utc::now(),
                    containers: Vec::new(),
                })
                .into_response();
            }
        };
        let metadata = parse_docker_inspect_metadata(&String::from_utf8_lossy(&inspect));
        for container in &mut containers {
            if let Some(metadata) = metadata.get(&container.id) {
                container.created_at = metadata.created_at.clone();
                container.image_id = metadata.image_id.clone();
                container.restart_count = metadata.restart_count;
                container.health = metadata.health.clone();
                container.oom_killed = metadata.oom_killed;
                container.started_at = metadata.started_at.clone();
            }
        }
    }
    Json(DockerDiscovery {
        available: true,
        reason: None,
        collected_at: Utc::now(),
        containers,
    })
    .into_response()
}

pub(super) async fn agent_docker_lifecycle(
    State(state): State<LocalAgentState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<DockerLifecycleRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if state.info.platform != AgentPlatform::Linux {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({"error":"unsupported_platform"})),
        )
            .into_response();
    }
    if !is_safe_docker_container_id(&request.container_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"invalid_container_id"})),
        )
            .into_response();
    }
    let action = match request.action {
        DockerLifecycleAction::Start => "start",
        DockerLifecycleAction::Stop => "stop",
        DockerLifecycleAction::Restart => "restart",
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.docker_output(&[action.to_owned(), request.container_id.clone()], None),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.success => Json(DockerLifecycleResult {
            container_id: request.container_id,
            action: request.action,
            completed_at: Utc::now(),
        })
        .into_response(),
        Ok(Ok(output)) => {
            let reason = docker_failure_reason(&output.stderr, None);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":reason})),
            )
                .into_response()
        }
        Ok(Err(error)) => {
            let reason = docker_failure_reason(&[], Some(error.kind()));
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":reason})),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error":"docker_action_timeout"})),
        )
            .into_response(),
    }
}

pub(super) async fn agent_docker_image_update_check(
    State(state): State<LocalAgentState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<DockerImageUpdateRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if state.info.platform != AgentPlatform::Linux {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({"error":"unsupported_platform"})),
        )
            .into_response();
    }
    if !is_safe_docker_container_id(&request.container_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"invalid_container_id"})),
        )
            .into_response();
    }
    let inspect = match state
        .docker_output(
            &[
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.Config.Image}}\\t{{.Image}}".to_owned(),
                request.container_id.clone(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error":"container_unavailable"})),
            )
                .into_response();
        }
    };
    let Some((image, current_image_id)) = inspect.split_once('\t') else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error":"container_metadata_unavailable"})),
        )
            .into_response();
    };
    let image = image.trim();
    let current_image_id = current_image_id.trim();
    if !safe_image_reference(image) || !is_safe_image_id(current_image_id) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error":"container_metadata_invalid"})),
        )
            .into_response();
    }
    let mut result = DockerImageUpdateResult {
        container_id: request.container_id,
        image: image.to_owned(),
        current_image_id: Some(current_image_id.to_owned()),
        remote_image_id: None,
        status: DockerImageUpdateStatus::Unknown,
        reason: None,
        checked_at: Utc::now(),
    };
    if image.contains("@sha256:") {
        result.status = DockerImageUpdateStatus::Pinned;
        result.reason = Some("digest_pinned".to_owned());
        return Json(result).into_response();
    }
    let architecture = match state
        .docker_output(
            &[
                "image".to_owned(),
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.Os}}/{{.Architecture}}".to_owned(),
                current_image_id.to_owned(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => {
            result.reason = Some("local_platform_unavailable".to_owned());
            return Json(result).into_response();
        }
    };
    let manifest = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.docker_output(
            &[
                "manifest".to_owned(),
                "inspect".to_owned(),
                "--verbose".to_owned(),
                image.to_owned(),
            ],
            None,
        ),
    )
    .await
    {
        Ok(Ok(output)) if output.success => output.stdout,
        _ => {
            result.reason = Some("registry_unavailable_or_unauthorized".to_owned());
            return Json(result).into_response();
        }
    };
    let Some(remote_image_id) =
        parse_remote_image_config_digest(&String::from_utf8_lossy(&manifest), &architecture)
    else {
        result.reason = Some("remote_platform_digest_unavailable".to_owned());
        return Json(result).into_response();
    };
    result.status = if remote_image_id == current_image_id {
        DockerImageUpdateStatus::Current
    } else {
        DockerImageUpdateStatus::UpdateAvailable
    };
    result.remote_image_id = Some(remote_image_id);
    Json(result).into_response()
}

pub(super) async fn agent_docker_image_update_apply(
    State(state): State<LocalAgentState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<DockerImageUpdateApplyRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if state.info.platform != AgentPlatform::Linux {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({"error":"unsupported_platform"})),
        )
            .into_response();
    }
    if !is_safe_docker_container_id(&request.container_id)
        || !is_safe_image_id(&request.expected_remote_image_id)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"invalid_update_request"})),
        )
            .into_response();
    }
    let inspected = match inspect_compose_update_container(&state, &request.container_id).await {
        Ok(inspected) => inspected,
        Err(response) => return response,
    };
    let remote_image_id =
        match verify_remote_update_image(&state, &inspected, &request.expected_remote_image_id)
            .await
        {
            Ok(image_id) => image_id,
            Err(response) => return response,
        };
    let service_status =
        match apply_compose_image_update(&state, &inspected, &remote_image_id).await {
            Ok(status) => status,
            Err(response) => return response,
        };
    Json(DockerImageUpdateApplyResult {
        container_id: request.container_id,
        image: inspected.image,
        compose_project: inspected.compose.project,
        compose_service: inspected.compose.service,
        image_id: remote_image_id,
        service_state: service_status.state,
        health_status: service_status.health,
        completed_at: Utc::now(),
    })
    .into_response()
}

struct InspectedDockerUpdateContainer {
    container_id: String,
    image: String,
    image_id: String,
    compose: ComposeContext,
}

async fn inspect_compose_update_container(
    state: &LocalAgentState,
    container_id: &str,
) -> Result<InspectedDockerUpdateContainer, axum::response::Response> {
    let inspect = match state
        .docker_output(
            &[
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.Id}}\t{{.Config.Image}}\t{{.Image}}\t{{json .Config.Labels}}".to_owned(),
                container_id.to_owned(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => String::from_utf8_lossy(&output.stdout).to_string(),
        _ => {
            return Err(docker_update_error(
                StatusCode::NOT_FOUND,
                "container_unavailable",
            ));
        }
    };
    let fields = inspect.trim().splitn(4, '\t').collect::<Vec<_>>();
    if fields.len() != 4
        || !fields[0].starts_with(container_id)
        || !is_safe_image_id(fields[2])
        || !safe_image_reference(fields[1])
    {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "container_metadata_invalid",
        ));
    }
    let labels: Value = serde_json::from_str(fields[3])
        .map_err(|_| docker_update_error(StatusCode::BAD_GATEWAY, "compose_labels_invalid"))?;
    let compose = validated_compose_context(&labels)
        .ok_or_else(|| docker_update_error(StatusCode::CONFLICT, "compose_metadata_required"))?;
    if labels
        .get("com.docker.compose.container-number")
        .and_then(Value::as_str)
        != Some("1")
    {
        return Err(docker_update_error(
            StatusCode::CONFLICT,
            "compose_replicas_unsupported",
        ));
    }
    let saved_config_hash = labels
        .get("com.docker.compose.config-hash")
        .and_then(Value::as_str)
        .ok_or_else(|| docker_update_error(StatusCode::CONFLICT, "compose_config_hash_required"))?
        .to_owned();
    let config_hash = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.docker_output(
            &compose.args(&["config", "--hash", &compose.service]),
            Some(&compose.working_dir),
        ),
    )
    .await;
    let Ok(Ok(output)) = config_hash else {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "compose_config_inspection_failed",
        ));
    };
    if !output.success || String::from_utf8_lossy(&output.stdout).trim() != saved_config_hash {
        return Err(docker_update_error(
            StatusCode::CONFLICT,
            "compose_config_changed",
        ));
    }
    Ok(InspectedDockerUpdateContainer {
        container_id: fields[0].to_owned(),
        image: fields[1].to_owned(),
        image_id: fields[2].to_owned(),
        compose,
    })
}

async fn verify_remote_update_image(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
    expected_remote_image_id: &str,
) -> Result<String, axum::response::Response> {
    let architecture = match state
        .docker_output(
            &[
                "image".to_owned(),
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.Os}}/{{.Architecture}}".to_owned(),
                container.image_id.clone(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => {
            return Err(docker_update_error(
                StatusCode::BAD_GATEWAY,
                "local_platform_unavailable",
            ));
        }
    };
    let manifest = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.docker_output(
            &[
                "manifest".to_owned(),
                "inspect".to_owned(),
                "--verbose".to_owned(),
                container.image.clone(),
            ],
            None,
        ),
    )
    .await
    {
        Ok(Ok(output)) if output.success => output.stdout,
        _ => {
            return Err(docker_update_error(
                StatusCode::BAD_GATEWAY,
                "registry_unavailable_or_unauthorized",
            ));
        }
    };
    let remote_image_id =
        parse_remote_image_config_digest(&String::from_utf8_lossy(&manifest), &architecture)
            .ok_or_else(|| {
                docker_update_error(
                    StatusCode::BAD_GATEWAY,
                    "remote_platform_digest_unavailable",
                )
            })?;
    if remote_image_id != expected_remote_image_id || remote_image_id == container.image_id {
        return Err(docker_update_error(
            StatusCode::CONFLICT,
            "image_update_check_stale",
        ));
    }
    Ok(remote_image_id)
}

async fn apply_compose_image_update(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
    remote_image_id: &str,
) -> Result<compose_update_status::ComposeServiceStatus, axum::response::Response> {
    let pull = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        state.docker_output(
            &container
                .compose
                .args(&["pull", &container.compose.service]),
            Some(&container.compose.working_dir),
        ),
    )
    .await;
    if !matches!(pull, Ok(Ok(ref output)) if output.success) {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "compose_pull_failed",
        ));
    }
    verify_pulled_image(state, container, remote_image_id).await?;
    verify_compose_service_scope(state, container).await?;
    recreate_compose_service(state, container).await?;
    compose_update_status::inspect_updated_compose_service(state, container).await
}

async fn verify_pulled_image(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
    remote_image_id: &str,
) -> Result<(), axum::response::Response> {
    let pulled = match state
        .docker_output(
            &[
                "image".to_owned(),
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.Id}}".to_owned(),
                container.image.clone(),
            ],
            None,
        )
        .await
    {
        Ok(output) if output.success => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => {
            return Err(docker_update_error(
                StatusCode::BAD_GATEWAY,
                "pulled_image_unavailable",
            ));
        }
    };
    if pulled != remote_image_id {
        return Err(docker_update_error(
            StatusCode::CONFLICT,
            "pulled_image_digest_mismatch",
        ));
    }
    Ok(())
}

async fn verify_compose_service_scope(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
) -> Result<(), axum::response::Response> {
    let service_containers = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.docker_output(
            &container
                .compose
                .args(&["ps", "--all", "--quiet", &container.compose.service]),
            Some(&container.compose.working_dir),
        ),
    )
    .await;
    let Ok(Ok(output)) = service_containers else {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "compose_service_inspection_failed",
        ));
    };
    let service_output = String::from_utf8_lossy(&output.stdout);
    let ids = service_output
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if !output.success || ids.len() != 1 || !ids[0].starts_with(&container.container_id) {
        return Err(docker_update_error(
            StatusCode::CONFLICT,
            "compose_service_scope_changed",
        ));
    }
    Ok(())
}

async fn recreate_compose_service(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
) -> Result<(), axum::response::Response> {
    let apply = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        state.docker_output(
            &container.compose.args(&[
                "up",
                "--detach",
                "--no-deps",
                "--force-recreate",
                "--wait",
                "--wait-timeout",
                "120",
                &container.compose.service,
            ]),
            Some(&container.compose.working_dir),
        ),
    )
    .await;
    if !matches!(apply, Ok(Ok(ref output)) if output.success) {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "compose_recreate_failed",
        ));
    }
    Ok(())
}

struct ComposeContext {
    project: String,
    service: String,
    working_dir: PathBuf,
    config_file: PathBuf,
}

impl ComposeContext {
    fn args(&self, operation: &[&str]) -> Vec<String> {
        let mut args = vec![
            "compose".to_owned(),
            "--project-name".to_owned(),
            self.project.clone(),
            "--project-directory".to_owned(),
            self.working_dir.to_string_lossy().into_owned(),
            "--file".to_owned(),
            self.config_file.to_string_lossy().into_owned(),
        ];
        args.extend(operation.iter().map(|value| (*value).to_owned()));
        args
    }
}

fn validated_compose_context(labels: &Value) -> Option<ComposeContext> {
    let label = |name: &str| labels.get(name).and_then(Value::as_str);
    let project = label("com.docker.compose.project")?;
    let service = label("com.docker.compose.service")?;
    let working_dir = label("com.docker.compose.project.working_dir")?;
    let config_files = label("com.docker.compose.project.config_files")?;
    if !safe_compose_identifier(project) || !safe_compose_identifier(service) {
        return None;
    }
    let mut files = config_files.split(',');
    let config_file = files.next()?.trim();
    if files.next().is_some() {
        return None;
    }
    let root = Path::new(working_dir).canonicalize().ok()?;
    let file = Path::new(config_file).canonicalize().ok()?;
    if !root.is_dir() || !file.is_file() || !file.starts_with(&root) {
        return None;
    }
    Some(ComposeContext {
        project: project.to_owned(),
        service: service.to_owned(),
        working_dir: root,
        config_file: file,
    })
}

fn safe_compose_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn docker_update_error(status: StatusCode, error: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({"error":error}))).into_response()
}

fn safe_image_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/:@+-".contains(&byte))
}

fn is_safe_image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod compose_validation_tests;
