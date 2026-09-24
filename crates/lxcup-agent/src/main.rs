use std::sync::Arc;

use lxcup_agent::{
    AgentHeartbeat, AgentInfo, AgentPlatform, LocalAgentState, SystemTelemetrySample, agent_router,
};

fn value_or_default(value: Option<String>, default: &str) -> String {
    value.unwrap_or_else(|| default.to_owned())
}

fn heartbeat_endpoint(controller_url: &str) -> String {
    format!(
        "{}/api/v1/agents/heartbeat",
        controller_url.trim_end_matches('/')
    )
}

async fn collect_telemetry() -> SystemTelemetrySample {
    let now = chrono::Utc::now();
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
        let storage_basis_points = tokio::process::Command::new("df")
            .args(["-Pk", "/"])
            .output()
            .await
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| {
                text.lines()
                    .nth(1)?
                    .split_whitespace()
                    .nth(4)?
                    .trim_end_matches('%')
                    .parse::<u16>()
                    .ok()
            })
            .map(|value| value.saturating_mul(100));
        let process_count = std::fs::read_dir("/proc").ok().map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .chars()
                        .all(|ch| ch.is_ascii_digit())
                })
                .count() as u32
        });
        return telemetry_from_proc(
            &meminfo,
            &loadavg,
            &stat,
            storage_basis_points,
            process_count,
            now,
        );
    }
    #[cfg(not(target_os = "linux"))]
    SystemTelemetrySample {
        collected_at: now,
        cpu_basis_points: None,
        memory_basis_points: None,
        storage_basis_points: None,
        load_1_milli: None,
        network_rx_bytes: None,
        network_tx_bytes: None,
        process_count: None,
    }
}

fn telemetry_from_proc(
    meminfo: &str,
    loadavg: &str,
    stat: &str,
    storage_basis_points: Option<u16>,
    process_count: Option<u32>,
    collected_at: chrono::DateTime<chrono::Utc>,
) -> SystemTelemetrySample {
    let value = |key: &str| {
        meminfo.lines().find_map(|line| {
            line.strip_prefix(key)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
    };
    let memory_basis_points =
        value("MemTotal:")
            .zip(value("MemAvailable:"))
            .map(|(total, available)| {
                (total.saturating_sub(available).saturating_mul(10_000) / total.max(1)).min(10_000)
                    as u16
            });
    let load_1_milli = loadavg
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f32>().ok())
        .map(|value| (value * 1000.0) as u32);
    let cpu_basis_points = stat
        .lines()
        .find(|line| line.starts_with("cpu "))
        .and_then(|line| {
            let values = line
                .split_whitespace()
                .skip(1)
                .filter_map(|value| value.parse::<u64>().ok())
                .collect::<Vec<_>>();
            let total = values.iter().sum::<u64>();
            if values.is_empty() {
                return None;
            }
            let idle = values
                .get(3)
                .copied()
                .unwrap_or_default()
                .saturating_add(values.get(4).copied().unwrap_or_default());
            Some(
                (total.saturating_sub(idle).saturating_mul(10_000) / total.max(1)).min(10_000)
                    as u16,
            )
        });
    SystemTelemetrySample {
        collected_at,
        cpu_basis_points,
        memory_basis_points,
        storage_basis_points,
        load_1_milli,
        network_rx_bytes: None,
        network_tx_bytes: None,
        process_count,
    }
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
    run(config, std::future::pending())
        .await
        .expect("agent must run");
}

async fn run(
    config: StartupConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let state = LocalAgentState::new(config.info.clone(), Arc::<str>::from(config.token.clone()));
    let telemetry_state = state.clone();
    tokio::spawn(async move {
        loop {
            telemetry_state
                .record_telemetry(collect_telemetry().await)
                .await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
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
                    telemetry: reporter_state.telemetry_window().await,
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
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        heartbeat_endpoint, run, startup_config_from_values, telemetry_from_proc, value_or_default,
    };

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

    #[test]
    fn telemetry_parser_handles_valid_and_malformed_proc_samples() {
        let now = chrono::Utc::now();
        let sample = telemetry_from_proc(
            "MemTotal: 1000 kB\nMemAvailable: 250 kB\n",
            "1.25 0.50 0.25 1/10 20",
            "cpu 10 0 20 60 10 0 0 0",
            Some(7_500),
            Some(42),
            now,
        );
        assert_eq!(sample.collected_at, now);
        assert_eq!(sample.memory_basis_points, Some(7_500));
        assert_eq!(sample.load_1_milli, Some(1_250));
        assert_eq!(sample.cpu_basis_points, Some(3_000));
        assert_eq!(sample.storage_basis_points, Some(7_500));
        assert_eq!(sample.process_count, Some(42));

        let malformed = telemetry_from_proc("MemTotal: nope", "", "cpu invalid", None, None, now);
        assert_eq!(malformed.memory_basis_points, None);
        assert_eq!(malformed.load_1_milli, None);
        assert_eq!(malformed.cpu_basis_points, None);
        assert_eq!(malformed.storage_basis_points, None);
    }

    #[tokio::test]
    async fn agent_runtime_starts_with_and_without_outbound_heartbeat() {
        for (controller, target_id) in [
            (None, None),
            (
                Some("http://127.0.0.1:1".to_owned()),
                Some("00000000-0000-0000-0000-000000000001".to_owned()),
            ),
        ] {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let bind = probe.local_addr().unwrap().to_string();
            drop(probe);
            let config = startup_config_from_values(
                Some("test-agent-token".to_owned()),
                Some("test-agent".to_owned()),
                Some("test-host".to_owned()),
                Some(bind),
                controller,
                target_id,
            )
            .unwrap();
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(run(config, async move {
                let _ = shutdown_rx.await;
            }));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            shutdown_tx.send(()).unwrap();
            server.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn telemetry_collection_returns_a_timestamped_sample() {
        let before = chrono::Utc::now();
        let sample = super::collect_telemetry().await;
        assert!(sample.collected_at >= before);
        assert!(sample.collected_at <= chrono::Utc::now());
    }
}
