use lxcup_agent::{
    AgentAction, AgentClient, AgentClientConfig, AgentCommandRequest, PROTOCOL_VERSION,
};

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set for the dedicated agent test"))
}

#[tokio::test]
#[ignore = "requires an explicitly configured, dedicated integration agent"]
async fn dedicated_agent_reports_health_and_records_health_commands() {
    let endpoint = required("LXCUP_INTEGRATION_AGENT_URL");
    let token = required("LXCUP_INTEGRATION_AGENT_TOKEN");
    let client = AgentClient::new(
        AgentClientConfig::new(endpoint, token)
            .expect("integration agent endpoint and token are valid"),
    )
    .expect("integration agent client can be built");

    let before = client
        .metrics()
        .await
        .expect("dedicated agent metrics endpoint is reachable");
    let health = client
        .health()
        .await
        .expect("dedicated agent health endpoint is reachable");
    assert!(health.healthy);
    assert_eq!(health.info.protocol_version, PROTOCOL_VERSION);

    let response = client
        .command(&AgentCommandRequest {
            action: AgentAction::Health,
            packages: Vec::new(),
            idempotency_key: format!("integration-health-{}", uuid::Uuid::new_v4()),
        })
        .await
        .expect("dedicated agent health command succeeds");
    assert!(
        response.success,
        "agent health command failed: {response:?}"
    );
    assert_eq!(response.exit_code, 0);

    let after = client
        .metrics()
        .await
        .expect("dedicated agent metrics endpoint remains reachable");
    assert_eq!(after.commands_total, before.commands_total + 1);
    assert_eq!(after.commands_failed, before.commands_failed);
    assert!(after.last_command_at.is_some());
}
