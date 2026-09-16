use std::sync::Arc;

use lxcup_agent::{AgentInfo, AgentPlatform, LocalAgentState, agent_router};

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-agent");
    let token = std::env::var("LXCUP_AGENT_TOKEN").expect("LXCUP_AGENT_TOKEN must be configured");
    let platform = if cfg!(windows) {
        AgentPlatform::Windows
    } else {
        AgentPlatform::Linux
    };
    let info = AgentInfo {
        agent_id: std::env::var("LXCUP_AGENT_ID").unwrap_or_else(|_| "local-agent".to_owned()),
        platform,
        hostname: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned()),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let bind =
        std::env::var("LXCUP_AGENT_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8090".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("agent bind address must be available");
    tracing::info!(%bind, agent_id = %info.agent_id, "lxcup agent starting");
    axum::serve(
        listener,
        agent_router(LocalAgentState::new(info, Arc::<str>::from(token))),
    )
    .await
    .expect("agent must run");
}
