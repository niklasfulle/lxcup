use super::{ComposeContext, InspectedDockerUpdateContainer, docker_update_error};
use crate::{LocalAgentState, is_safe_docker_container_id};
use axum::{http::StatusCode, response::Response};

#[derive(Debug)]
pub(super) struct ComposeServiceStatus {
    pub(super) state: String,
    pub(super) health: Option<String>,
}

pub(super) async fn inspect_updated_compose_service(
    state: &LocalAgentState,
    container: &InspectedDockerUpdateContainer,
) -> Result<ComposeServiceStatus, Response> {
    inspect_service_status(state, &container.compose).await
}

async fn inspect_service_status(
    state: &LocalAgentState,
    compose: &ComposeContext,
) -> Result<ComposeServiceStatus, Response> {
    let listing = state
        .docker_output(
            &compose.args(&["ps", "--all", "--quiet", &compose.service]),
            Some(&compose.working_dir),
        )
        .await;
    let Ok(output) = listing else {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "updated_service_inspection_failed",
        ));
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let ids = listing
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if !output.success || ids.len() != 1 || !is_safe_docker_container_id(ids[0]) {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "updated_service_scope_invalid",
        ));
    }
    let inspected = state
        .docker_output(
            &[
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.State.Status}}\t{{if .State.Health}}{{.State.Health.Status}}{{end}}".to_owned(),
                ids[0].to_owned(),
            ],
            None,
        )
        .await;
    let Ok(inspected) = inspected else {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "updated_service_inspection_failed",
        ));
    };
    let inspected_output = String::from_utf8_lossy(&inspected.stdout);
    let fields = inspected_output
        .trim()
        .split('\t')
        .map(str::trim)
        .collect::<Vec<_>>();
    if !inspected.success || fields.len() != 2 || fields[0] != "running" {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "updated_service_not_running",
        ));
    }
    let health = (!fields[1].is_empty()).then(|| fields[1].to_owned());
    if health.as_deref().is_some_and(|status| status != "healthy") {
        return Err(docker_update_error(
            StatusCode::BAD_GATEWAY,
            "updated_service_unhealthy",
        ));
    }
    Ok(ComposeServiceStatus {
        state: fields[0].to_owned(),
        health,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentInfo, AgentPlatform, PROTOCOL_VERSION};
    use std::sync::Arc;

    fn inspected_container() -> InspectedDockerUpdateContainer {
        let directory = std::env::temp_dir();
        InspectedDockerUpdateContainer {
            container_id: "a".repeat(64),
            image: "nginx:stable".to_owned(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            compose: ComposeContext {
                project: "shop".to_owned(),
                service: "web".to_owned(),
                working_dir: directory.clone(),
                config_file: directory.join("compose.yaml"),
            },
        }
    }

    fn state_with_outputs(
        outputs: impl IntoIterator<Item = crate::docker::DockerCommandOutput>,
    ) -> LocalAgentState {
        let runner = Arc::new(crate::tests::ScriptedDockerRunner::default());
        runner.outputs.lock().unwrap().extend(outputs);
        LocalAgentState::new(
            AgentInfo {
                agent_id: "linux-agent".to_owned(),
                platform: AgentPlatform::Linux,
                hostname: "linux-host".to_owned(),
                version: "0.4.0".to_owned(),
                protocol_version: PROTOCOL_VERSION.to_owned(),
            },
            "docker-token",
        )
        .with_docker_command_runner(runner)
    }

    #[tokio::test]
    async fn updated_service_inspection_rejects_unavailable_and_ambiguous_scope() {
        let container = inspected_container();
        let unavailable = state_with_outputs([]);
        let error = inspect_updated_compose_service(&unavailable, &container)
            .await
            .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);

        let multiple = state_with_outputs([crate::tests::docker_result(
            true,
            format!("{}\n{}\n", "a".repeat(64), "b".repeat(64)),
        )]);
        let error = inspect_updated_compose_service(&multiple, &container)
            .await
            .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn updated_service_inspection_requires_a_running_container() {
        let container = inspected_container();
        let not_running = state_with_outputs([
            crate::tests::docker_result(true, format!("{}\n", "a".repeat(64))),
            crate::tests::docker_result(true, "exited\t\n"),
        ]);
        let error = inspect_updated_compose_service(&not_running, &container)
            .await
            .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);

        let inspect_failed = state_with_outputs([
            crate::tests::docker_result(true, format!("{}\n", "a".repeat(64))),
            crate::tests::docker_result(false, ""),
        ]);
        let error = inspect_updated_compose_service(&inspect_failed, &container)
            .await
            .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
    }
}
