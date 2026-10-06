use lxcup_agent::{
    AgentHeartbeat, AgentInfo, AgentPlatform, AgentWorkflowClaimRequest, AgentWorkflowCommand,
    AgentWorkflowResult, LocalAgentState, execute_workflow_command,
};

pub(crate) fn heartbeat_endpoint(controller_url: &str) -> String {
    format!(
        "{}/api/v1/agents/heartbeat",
        controller_url.trim_end_matches('/')
    )
}

fn workflow_claim_endpoint(controller_url: &str) -> String {
    format!(
        "{}/api/v1/agents/workflows/claim",
        controller_url.trim_end_matches('/')
    )
}

fn workflow_result_endpoint(controller_url: &str, job_id: uuid::Uuid) -> String {
    format!(
        "{}/api/v1/agents/workflows/{job_id}/result",
        controller_url.trim_end_matches('/')
    )
}

pub(crate) async fn send_agent_heartbeat(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    target_id: uuid::Uuid,
    info: &AgentInfo,
    state: &LocalAgentState,
) -> bool {
    let heartbeat = AgentHeartbeat {
        target_id,
        info: info.clone(),
        metrics: state.metrics_snapshot().await,
        sent_at: chrono::Utc::now(),
        telemetry: state.telemetry_window().await,
        docker_telemetry: state.docker_telemetry_window().await,
        package_inventory: state.package_inventory_snapshot().await,
    };
    let inventory_collected_at = heartbeat
        .package_inventory
        .as_ref()
        .map(|inventory| inventory.collected_at);
    let sample_count = heartbeat.telemetry.samples.len();
    match client
        .post(endpoint)
        .bearer_auth(token)
        .json(&heartbeat)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            if let Some(collected_at) = inventory_collected_at {
                state.acknowledge_package_inventory(collected_at).await;
            }
            true
        }
        Ok(response) => {
            tracing::warn!(
                target: "lxcup_agent::telemetry",
                samples = sample_count,
                status = %response.status(),
                "agent heartbeat rejected by controller"
            );
            false
        }
        Err(error) => {
            tracing::warn!(
                target: "lxcup_agent::telemetry",
                samples = sample_count,
                %error,
                "outbound agent heartbeat failed"
            );
            false
        }
    }
}

pub(crate) async fn run_windows_workflow_poller(
    controller: String,
    token: String,
    state: LocalAgentState,
    target_id: uuid::Uuid,
    info: AgentInfo,
) {
    let client = reqwest::Client::new();
    let endpoint = workflow_claim_endpoint(&controller);
    loop {
        let claim = client
            .post(&endpoint)
            .bearer_auth(&token)
            .json(&AgentWorkflowClaimRequest { target_id })
            .send()
            .await;
        match claim {
            Ok(response) if response.status().is_success() => {
                match response.json::<WorkflowClaimEnvelope>().await {
                    Ok(envelope) => {
                        if let Some(command) = envelope.data {
                            report_claimed_workflow(
                                &client,
                                &controller,
                                &token,
                                &state,
                                target_id,
                                &info,
                                command,
                            )
                            .await;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Windows workflow claim response was invalid")
                    }
                }
            }
            Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT => {}
            Ok(response) => {
                tracing::warn!(status = %response.status(), "Windows workflow claim was rejected")
            }
            Err(error) => tracing::warn!(%error, "outbound Windows workflow claim failed"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

#[derive(serde::Deserialize)]
struct WorkflowClaimEnvelope {
    data: Option<AgentWorkflowCommand>,
}

async fn report_claimed_workflow(
    client: &reqwest::Client,
    controller: &str,
    token: &str,
    state: &LocalAgentState,
    target_id: uuid::Uuid,
    info: &AgentInfo,
    command: AgentWorkflowCommand,
) {
    if command.action == lxcup_agent::AgentAction::UpdateAgent {
        #[cfg(windows)]
        {
            let version = command.agent_version.as_deref().unwrap_or_default();
            match crate::windows_service::spawn_agent_update(command.job_id, version) {
                Ok(()) => return,
                Err(error) => {
                    tracing::error!(job_id = %command.job_id, %error, "could not start Windows agent updater");
                }
            }
        }
        #[cfg(not(windows))]
        tracing::error!(job_id = %command.job_id, "Windows agent update requested on a non-Windows host");
    }
    let mut result = execute_workflow_command(state, &command).await;
    let inventory_workflow =
        command.action == lxcup_agent::AgentAction::Scan && command.packages.is_empty();
    if result.response.success && should_collect_package_inventory(&command) {
        match lxcup_agent::collect_package_inventory(AgentPlatform::Windows).await {
            Ok(inventory) => {
                state.record_package_inventory(inventory).await;
                let endpoint = heartbeat_endpoint(controller);
                let _ =
                    send_agent_heartbeat(client, &endpoint, token, target_id, info, state).await;
            }
            Err(error) => {
                tracing::warn!(?error, "post-workflow winget inventory collection failed");
                if inventory_workflow {
                    result.response.success = false;
                    result.response.exit_code = 1;
                    result.response.stdout.clear();
                    result.response.stderr = error.code().to_owned();
                }
            }
        }
    }
    let endpoint = workflow_result_endpoint(controller, command.job_id);
    loop {
        match client
            .post(&endpoint)
            .bearer_auth(token)
            .json(&AgentWorkflowResult {
                response: result.response.clone(),
            })
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => break,
            Ok(response) => {
                tracing::warn!(job_id = %command.job_id, status = %response.status(), "Windows workflow result was rejected; retrying")
            }
            Err(error) => {
                tracing::warn!(job_id = %command.job_id, %error, "Windows workflow result delivery failed; retrying")
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

fn should_collect_package_inventory(command: &AgentWorkflowCommand) -> bool {
    command.action == lxcup_agent::AgentAction::Apply
        || (command.action == lxcup_agent::AgentAction::Scan && command.packages.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        heartbeat_endpoint, send_agent_heartbeat, should_collect_package_inventory,
        workflow_claim_endpoint, workflow_result_endpoint,
    };
    use lxcup_agent::{AgentWorkflowCommand, LocalAgentState};

    #[test]
    fn package_inventory_workflow_refreshes_inventory_but_update_plan_does_not() {
        let inventory = AgentWorkflowCommand {
            job_id: uuid::Uuid::new_v4(),
            action: lxcup_agent::AgentAction::Scan,
            packages: Vec::new(),
            agent_version: None,
        };
        let update_plan = AgentWorkflowCommand {
            packages: vec!["7zip.7zip".to_owned()],
            ..inventory.clone()
        };
        let update_apply = AgentWorkflowCommand {
            action: lxcup_agent::AgentAction::Apply,
            ..update_plan.clone()
        };
        assert!(should_collect_package_inventory(&inventory));
        assert!(!should_collect_package_inventory(&update_plan));
        assert!(should_collect_package_inventory(&update_apply));
    }

    #[test]
    fn outbound_agent_endpoints_normalize_controller_slashes() {
        let job_id = uuid::Uuid::nil();
        assert_eq!(
            heartbeat_endpoint("http://controller/"),
            "http://controller/api/v1/agents/heartbeat"
        );
        assert_eq!(
            workflow_claim_endpoint("https://controller/"),
            "https://controller/api/v1/agents/workflows/claim"
        );
        assert_eq!(
            workflow_result_endpoint("https://controller/", job_id),
            format!("https://controller/api/v1/agents/workflows/{job_id}/result")
        );
    }

    #[tokio::test]
    async fn agent_heartbeat_sends_telemetry_and_package_inventory_to_controller() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let sender = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
        let app = axum::Router::new().route(
            "/api/v1/agents/heartbeat",
            axum::routing::post({
                let sender = sender.clone();
                move |axum::Json(heartbeat): axum::Json<lxcup_agent::AgentHeartbeat>| {
                    let sender = sender.lock().unwrap().take();
                    async move {
                        if let Some(sender) = sender {
                            let _ = sender.send(heartbeat);
                        }
                        axum::http::StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/api/v1/agents/heartbeat",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let now = chrono::Utc::now();
        let info = lxcup_agent::AgentInfo {
            agent_id: "heartbeat-test".to_owned(),
            platform: lxcup_agent::AgentPlatform::Windows,
            hostname: "windows-test".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        };
        let target_id = uuid::Uuid::new_v4();
        let state = LocalAgentState::new(info.clone(), "test-token");
        state
            .record_telemetry(crate::telemetry_from_windows_json(
                r#"{"cpu_percent":25,"memory_percent":45,"storage_percent":50,"rx_bytes":12,"tx_bytes":24,"process_count":30}"#,
                now,
            ))
            .await;
        state
            .record_package_inventory(lxcup_agent::AgentPackageInventory {
                collected_at: now,
                packages: vec![lxcup_agent::AgentInstalledPackage {
                    name: "7zip.7zip".to_owned(),
                    installed_version: "24.09".to_owned(),
                    candidate_version: None,
                    architecture: None,
                    source: Some("winget".to_owned()),
                }],
            })
            .await;

        assert!(
            send_agent_heartbeat(
                &reqwest::Client::new(),
                &endpoint,
                "test-token",
                target_id,
                &info,
                &state
            )
            .await
        );
        let heartbeat = receiver.await.unwrap();
        assert_eq!(heartbeat.target_id, target_id);
        assert_eq!(heartbeat.telemetry.samples[0].cpu_basis_points, Some(2_500));
        assert_eq!(
            heartbeat.telemetry.samples[0].memory_basis_points,
            Some(4_500)
        );
        assert_eq!(
            heartbeat.telemetry.samples[0].storage_basis_points,
            Some(5_000)
        );
        assert_eq!(heartbeat.telemetry.samples[0].network_rx_bytes, Some(12));
        assert_eq!(heartbeat.telemetry.samples[0].network_tx_bytes, Some(24));
        assert_eq!(heartbeat.telemetry.samples[0].process_count, Some(30));
        assert_eq!(
            heartbeat.package_inventory.unwrap().packages[0].name,
            "7zip.7zip"
        );
        assert!(state.package_inventory_snapshot().await.is_none());
        server.abort();
    }

    #[test]
    fn windows_telemetry_keeps_available_metrics_when_some_collectors_fail() {
        let sample = crate::telemetry_from_windows_json(
            r#"{"cpu_percent":null,"memory_percent":43.25,"storage_percent":null,"rx_bytes":1200,"tx_bytes":null,"process_count":87}"#,
            chrono::Utc::now(),
        );

        assert_eq!(sample.cpu_basis_points, None);
        assert_eq!(sample.memory_basis_points, Some(4_325));
        assert_eq!(sample.storage_basis_points, None);
        assert_eq!(sample.network_rx_bytes, Some(1_200));
        assert_eq!(sample.network_tx_bytes, None);
        assert_eq!(sample.process_count, Some(87));
    }
}
