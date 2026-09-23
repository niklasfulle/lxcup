use std::sync::Arc;

use lxcup_agent::{AgentHeartbeat, AgentInfo, AgentPlatform, LocalAgentState, agent_router};

fn value_or_default(value: Option<String>, default: &str) -> String {
    value.unwrap_or_else(|| default.to_owned())
}

fn heartbeat_endpoint(controller_url: &str) -> String {
    format!(
        "{}/api/v1/agents/heartbeat",
        controller_url.trim_end_matches('/')
    )
}

struct StartupConfig {
    token: String,
    info: AgentInfo,
    bind: String,
    heartbeat: Option<(String, uuid::Uuid)>,
}

fn startup_config_from_values(
    token: Option<String>,
    agent_id: Option<String>,
    hostname: Option<String>,
    bind: Option<String>,
    controller_url: Option<String>,
    target_id: Option<String>,
) -> Result<StartupConfig, &'static str> {
    let token = token
        .filter(|value| !value.trim().is_empty())
        .ok_or("LXCUP_AGENT_TOKEN must be configured")?;
    let heartbeat = match (controller_url, target_id) {
        (Some(controller), Some(target)) => uuid::Uuid::parse_str(&target)
            .ok()
            .map(|target_id| (controller, target_id)),
        _ => None,
    };
    Ok(StartupConfig {
        token,
        info: AgentInfo {
            agent_id: value_or_default(agent_id, "local-agent"),
            platform: if cfg!(windows) {
                AgentPlatform::Windows
            } else {
                AgentPlatform::Linux
            },
            hostname: value_or_default(hostname, "unknown"),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        },
        bind: value_or_default(bind, "127.0.0.1:8090"),
        heartbeat,
    })
}

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-agent");
    let config = startup_config_from_values(
        std::env::var("LXCUP_AGENT_TOKEN").ok(),
        std::env::var("LXCUP_AGENT_ID").ok(),
        std::env::var("HOSTNAME").ok(),
        std::env::var("LXCUP_AGENT_BIND_ADDRESS").ok(),
        std::env::var("LXCUP_CONTROLLER_URL").ok(),
        std::env::var("LXCUP_TARGET_ID").ok(),
    )
    .expect("worker configuration");
    let state = LocalAgentState::new(config.info.clone(), Arc::<str>::from(config.token.clone()));
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .expect("agent bind address must be available");
    tracing::info!(%config.bind, agent_id = %config.info.agent_id, "lxcup agent starting");
    if let Some((controller_url, target_id)) = config.heartbeat {
        let reporter = reqwest::Client::new();
        let reporter_info = config.info.clone();
        let reporter_token = config.token.clone();
        let reporter_state = state.clone();
        tokio::spawn(async move {
            let endpoint = heartbeat_endpoint(&controller_url);
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
    }
    axum::serve(listener, agent_router(state))
        .await
        .expect("agent must run");
}

#[cfg(test)]
mod tests {
    use super::{heartbeat_endpoint, startup_config_from_values, value_or_default};

    #[test]
    fn heartbeat_endpoint_normalizes_controller_slashes() {
        assert_eq!(
            heartbeat_endpoint("http://controller/"),
            "http://controller/api/v1/agents/heartbeat"
        );
        assert_eq!(
            heartbeat_endpoint("http://controller"),
            "http://controller/api/v1/agents/heartbeat"
        );
    }

    #[test]
    fn environment_values_fall_back_only_when_missing() {
        assert_eq!(
            value_or_default(Some("configured".to_owned()), "fallback"),
            "configured"
        );
        assert_eq!(value_or_default(None, "fallback"), "fallback");
    }

    #[test]
    fn startup_configuration_validates_token_and_optional_heartbeat_target() {
        assert!(startup_config_from_values(None, None, None, None, None, None).is_err());
        assert!(
            startup_config_from_values(Some("  ".to_owned()), None, None, None, None, None,)
                .is_err()
        );
        let config = startup_config_from_values(
            Some("token".to_owned()),
            Some("agent-1".to_owned()),
            Some("host-1".to_owned()),
            Some("127.0.0.1:9000".to_owned()),
            Some("http://controller/".to_owned()),
            Some("00000000-0000-0000-0000-000000000001".to_owned()),
        )
        .unwrap();
        assert_eq!(config.token, "token");
        assert_eq!(config.info.agent_id, "agent-1");
        assert_eq!(config.bind, "127.0.0.1:9000");
        assert!(config.heartbeat.is_some());
        let defaults =
            startup_config_from_values(Some("token".to_owned()), None, None, None, None, None)
                .unwrap();
        assert_eq!(defaults.info.agent_id, "local-agent");
        assert_eq!(defaults.info.hostname, "unknown");
        assert_eq!(defaults.bind, "127.0.0.1:8090");
        assert!(defaults.heartbeat.is_none());
        assert!(
            startup_config_from_values(
                Some("token".to_owned()),
                None,
                None,
                None,
                Some("http://controller".to_owned()),
                Some("invalid".to_owned()),
            )
            .unwrap()
            .heartbeat
            .is_none()
        );
    }
}
