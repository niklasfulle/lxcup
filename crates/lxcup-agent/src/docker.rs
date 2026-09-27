use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use tokio::process::Command;

use super::parsers::{docker_failure_reason, parse_docker_containers};
use super::{AgentPlatform, DockerDiscovery, LocalAgentState, authorized};

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

    let output = match Command::new("docker")
        .args([
            "ps",
            "--all",
            "--no-trunc",
            "--format",
            "{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.State}}\\t{{.Status}}\\t{{.Ports}}\\t{{.CreatedAt}}\\t{{.Labels}}",
        ])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output.stdout,
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

    let containers = parse_docker_containers(&String::from_utf8_lossy(&output));
    Json(DockerDiscovery {
        available: true,
        reason: None,
        collected_at: Utc::now(),
        containers,
    })
    .into_response()
}
