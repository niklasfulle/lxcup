use std::sync::Arc;

use lxcup_agent::{AgentHeartbeat, AgentInfo, AgentPlatform, LocalAgentState, agent_router};

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
    let state = LocalAgentState::new(info.clone(), Arc::<str>::from(token.clone()));
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("agent bind address must be available");
    tracing::info!(%bind, agent_id = %info.agent_id, "lxcup agent starting");
    if let (Ok(controller_url), Ok(target_id)) = (
        std::env::var("LXCUP_CONTROLLER_URL"),
        std::env::var("LXCUP_TARGET_ID"),
    ) {
        if let Ok(target_id) = uuid::Uuid::parse_str(&target_id) {
            let reporter = reqwest::Client::new();
            let reporter_info = info.clone();
            let reporter_token = token.clone();
            let reporter_state = state.clone();
            tokio::spawn(async move {
                let endpoint = format!(
                    "{}/api/v1/agents/heartbeat",
                    controller_url.trim_end_matches('/')
                );
                loop {
                    let heartbeat = AgentHeartbeat {
                        target_id,
                        info: reporter_info.clone(),
                        metrics: reporter_state.metrics_snapshot().await,
                        sent_at: chrono::Utc::now(),
                    };
                    if let Err(error) = reporter
                        .post(&endpoint)
                        .bearer_auth(&reporter_token)
                        .json(&heartbeat)
                        .send()
                        .await
                    {
                        tracing::warn!(%error, "outbound agent heartbeat failed");
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
        } else {
            tracing::warn!("LXCUP_TARGET_ID is invalid; outbound agent reporting is disabled");
        }
    }
    axum::serve(listener, agent_router(state))
        .await
        .expect("agent must run");
}
