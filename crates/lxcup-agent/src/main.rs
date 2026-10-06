use std::sync::Arc;
use tokio::process::Command;

#[cfg(windows)]
mod windows_service;
mod workflow_client;

use lxcup_agent::{
    AgentInfo, AgentPlatform, LocalAgentState, SystemTelemetrySample, agent_router,
    parse_docker_stats,
};
use workflow_client::{heartbeat_endpoint, send_agent_heartbeat};

fn value_or_default(value: Option<String>, default: &str) -> String {
    value.unwrap_or_else(|| default.to_owned())
}

async fn collect_docker_telemetry(
    platform: AgentPlatform,
) -> Vec<lxcup_agent::DockerTelemetrySample> {
    if platform != AgentPlatform::Linux {
        return Vec::new();
    }
    let output = match Command::new("docker")
        .args([
            "stats",
            "--all",
            "--no-stream",
            "--no-trunc",
            "--format",
            "{{.ID}}\\t{{.CPUPerc}}\\t{{.MemUsage}}\\t{{.MemPerc}}",
        ])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => return Vec::new(),
    };
    parse_docker_stats(&String::from_utf8_lossy(&output), chrono::Utc::now())
}

async fn collect_telemetry_with_cpu(
    previous_cpu: Option<(u64, u64)>,
) -> (SystemTelemetrySample, Option<(u64, u64)>) {
    let now = chrono::Utc::now();
    #[cfg(not(target_os = "linux"))]
    let _ = previous_cpu;
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
        let network = std::fs::read_to_string("/proc/net/dev").unwrap_or_default();
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
        return telemetry_from_proc(ProcTelemetryInput {
            meminfo: &meminfo,
            loadavg: &loadavg,
            stat: &stat,
            network: &network,
            storage_basis_points,
            process_count,
            previous_cpu,
            collected_at: now,
        });
    }
    #[cfg(target_os = "windows")]
    {
        (collect_windows_telemetry(now).await, None)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    (
        SystemTelemetrySample {
            collected_at: now,
            cpu_basis_points: None,
            memory_basis_points: None,
            storage_basis_points: None,
            load_1_milli: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            process_count: None,
        },
        None,
    )
}

#[cfg(target_os = "windows")]
async fn collect_windows_telemetry(now: chrono::DateTime<chrono::Utc>) -> SystemTelemetrySample {
    const COMMAND: &str = r#"$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Get-SafeMetric([scriptblock]$Collect) {
    try { & $Collect } catch { $null }
}
$cpu = Get-SafeMetric {
    $processors = @(Get-CimInstance Win32_Processor -ErrorAction Stop)
    if ($processors.Count -gt 0) {
        $average = ($processors | Measure-Object -Property LoadPercentage -Average).Average
        if ($null -ne $average) { $average }
    }
}
$memory = Get-SafeMetric {
    $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
    if ($os.TotalVisibleMemorySize -gt 0) {
        100 * ($os.TotalVisibleMemorySize - $os.FreePhysicalMemory) / $os.TotalVisibleMemorySize
    }
}
$storage = Get-SafeMetric {
    $disks = @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' -ErrorAction Stop | Where-Object { $_.Size -gt 0 })
    $total = ($disks | Measure-Object -Property Size -Sum).Sum
    $free = ($disks | Measure-Object -Property FreeSpace -Sum).Sum
    if ($total -gt 0) { 100 * ($total - $free) / $total }
}
$network = Get-SafeMetric {
    $adapters = @(Get-NetAdapterStatistics -ErrorAction Stop)
    if ($adapters.Count -gt 0) {
        [ordered]@{
            rx = [uint64](($adapters | Measure-Object -Property ReceivedBytes -Sum).Sum)
            tx = [uint64](($adapters | Measure-Object -Property SentBytes -Sum).Sum)
        }
    }
}
$processes = Get-SafeMetric { @(Get-Process -ErrorAction Stop).Count }
[ordered]@{
    cpu_percent = $cpu
    memory_percent = $memory
    storage_percent = $storage
    rx_bytes = if ($null -ne $network) { $network.rx } else { $null }
    tx_bytes = if ($null -ne $network) { $network.tx } else { $null }
    process_count = $processes
} | ConvertTo-Json -Compress -Depth 3"#;
    let output = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            COMMAND,
        ])
        .output()
        .await;
    match output {
        Ok(output) => {
            if !output.status.success() {
                tracing::warn!(status = ?output.status.code(), "Windows telemetry PowerShell exited unsuccessfully; retaining available metrics");
            }
            telemetry_from_windows_json(&String::from_utf8_lossy(&output.stdout), now)
        }
        Err(error) => {
            tracing::warn!(%error, "Windows telemetry PowerShell could not be started");
            telemetry_from_windows_json("{}", now)
        }
    }
}

fn telemetry_from_windows_json(
    output: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> SystemTelemetrySample {
    let output = serde_json::from_str::<serde_json::Value>(output).ok();
    let get = |name: &str| output.as_ref()?.get(name)?.as_f64();
    let integer = |name: &str| output.as_ref()?.get(name)?.as_u64();
    let percent =
        |name: &str| get(name).map(|value| (value.clamp(0.0, 100.0) * 100.0).round() as u16);
    SystemTelemetrySample {
        collected_at: now,
        cpu_basis_points: percent("cpu_percent"),
        memory_basis_points: percent("memory_percent"),
        storage_basis_points: percent("storage_percent"),
        load_1_milli: None,
        network_rx_bytes: integer("rx_bytes"),
        network_tx_bytes: integer("tx_bytes"),
        process_count: integer("process_count").map(|count| count.min(u32::MAX as u64) as u32),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct ProcTelemetryInput<'a> {
    meminfo: &'a str,
    loadavg: &'a str,
    stat: &'a str,
    network: &'a str,
    storage_basis_points: Option<u16>,
    process_count: Option<u32>,
    previous_cpu: Option<(u64, u64)>,
    collected_at: chrono::DateTime<chrono::Utc>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn telemetry_from_proc(
    input: ProcTelemetryInput<'_>,
) -> (SystemTelemetrySample, Option<(u64, u64)>) {
    let ProcTelemetryInput {
        meminfo,
        loadavg,
        stat,
        network,
        storage_basis_points,
        process_count,
        previous_cpu,
        collected_at,
    } = input;
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
    let cpu_snapshot = stat
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
            Some((total, idle))
        });
    let cpu_basis_points =
        cpu_snapshot
            .zip(previous_cpu)
            .and_then(|((total, idle), (last_total, last_idle))| {
                let total_delta = total.saturating_sub(last_total);
                let idle_delta = idle.saturating_sub(last_idle);
                (total_delta > 0).then(|| {
                    (total_delta
                        .saturating_sub(idle_delta)
                        .saturating_mul(10_000)
                        / total_delta)
                        .min(10_000) as u16
                })
            });
    let (network_rx_bytes, network_tx_bytes) = parse_network_counters(network);
    (
        SystemTelemetrySample {
            collected_at,
            cpu_basis_points,
            memory_basis_points,
            storage_basis_points,
            load_1_milli,
            network_rx_bytes,
            network_tx_bytes,
            process_count,
        },
        cpu_snapshot,
    )
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_network_counters(network: &str) -> (Option<u64>, Option<u64>) {
    let counters = network.lines().skip(2).filter_map(|line| {
        let (_, values) = line.split_once(':')?;
        let values = values.split_whitespace().collect::<Vec<_>>();
        Some((
            values.first()?.parse::<u64>().ok()?,
            values.get(8)?.parse::<u64>().ok()?,
        ))
    });
    let (rx, tx, count) = counters.fold(
        (0_u64, 0_u64, 0_u32),
        |(rx, tx, count), (next_rx, next_tx)| {
            (
                rx.saturating_add(next_rx),
                tx.saturating_add(next_tx),
                count.saturating_add(1),
            )
        },
    );
    ((count > 0).then_some(rx), (count > 0).then_some(tx))
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

fn startup_config_from_environment() -> Result<StartupConfig, &'static str> {
    #[cfg(windows)]
    let file_values = parse_agent_environment(
        &std::fs::read_to_string(r"C:\ProgramData\lxcup\agent.env")
            .map_err(|_| "agent service configuration is unavailable")?,
    )?;
    #[cfg(not(windows))]
    let file_values: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let value = |key: &str| {
        std::env::var(key)
            .ok()
            .or_else(|| file_values.get(key).cloned())
    };
    startup_config_from_values(
        value("LXCUP_AGENT_TOKEN"),
        value("LXCUP_AGENT_ID"),
        value("HOSTNAME").or_else(|| value("COMPUTERNAME")),
        value("LXCUP_AGENT_BIND_ADDRESS"),
        value("LXCUP_CONTROLLER_URL"),
        value("LXCUP_TARGET_ID"),
    )
}

#[cfg(any(windows, test))]
fn parse_agent_environment(
    contents: &str,
) -> Result<std::collections::HashMap<String, String>, &'static str> {
    let allowed = [
        "LXCUP_AGENT_TOKEN",
        "LXCUP_AGENT_ID",
        "LXCUP_TARGET_ID",
        "LXCUP_AGENT_BIND_ADDRESS",
        "LXCUP_CONTROLLER_URL",
        "LXCUP_TARGET_ID",
    ];
    let mut values = std::collections::HashMap::new();
    for line in contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (key, value) = line
            .split_once('=')
            .ok_or("agent service configuration is invalid")?;
        if !allowed.contains(&key) || value.trim().is_empty() || values.contains_key(key) {
            return Err("agent service configuration is invalid");
        }
        values.insert(key.to_owned(), value.to_owned());
    }
    Ok(values)
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-agent");
    let config = startup_config_from_environment().expect("agent configuration");
    run(config, std::future::pending())
        .await
        .expect("agent must run");
}

#[cfg(windows)]
#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-agent");
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--apply-agent-update")
    {
        if arguments.len() != 3 {
            eprintln!("invalid internal agent updater invocation");
            std::process::exit(2);
        }
        let job_id = match uuid::Uuid::parse_str(&arguments[1]) {
            Ok(job_id) => job_id,
            Err(_) => {
                eprintln!("invalid agent update workflow id");
                std::process::exit(2);
            }
        };
        let exit_code =
            windows_service::run_agent_update_helper(job_id, arguments[2].clone()).await;
        std::process::exit(exit_code);
    }
    if let Err(error) = windows_service::start() {
        windows_service::record_startup_failure("service dispatcher", &error);
        panic!("agent service dispatcher must start: {error}");
    }
}

async fn run(
    config: StartupConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let state = LocalAgentState::new(config.info.clone(), Arc::<str>::from(config.token.clone()));
    let telemetry_state = state.clone();
    tokio::spawn(async move {
        let mut previous_cpu = None;
        loop {
            let (sample, current_cpu) = collect_telemetry_with_cpu(previous_cpu).await;
            previous_cpu = current_cpu;
            telemetry_state.record_telemetry(sample).await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    let docker_telemetry_state = state.clone();
    let docker_platform = config.info.platform;
    tokio::spawn(async move {
        loop {
            let samples = collect_docker_telemetry(docker_platform).await;
            docker_telemetry_state
                .record_docker_telemetry(samples)
                .await;
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
    if config.info.platform == AgentPlatform::Windows {
        let inventory_state = state.clone();
        tokio::spawn(async move {
            loop {
                match lxcup_agent::collect_package_inventory(AgentPlatform::Windows).await {
                    Ok(inventory) => inventory_state.record_package_inventory(inventory).await,
                    Err(error) => {
                        tracing::warn!(?error, "Windows winget inventory collection failed")
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(900)).await;
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(%config.bind, agent_id = %config.info.agent_id, "lxcup agent starting");
    if let Some((controller_url, target_id)) = config.heartbeat {
        let reporter_controller = controller_url.clone();
        let workflow_controller = controller_url.clone();
        let reporter = reqwest::Client::new();
        let reporter_info = config.info.clone();
        let reporter_token = config.token.clone();
        let reporter_state = state.clone();
        let reporter_target_id = target_id;
        tokio::spawn(async move {
            let endpoint = heartbeat_endpoint(&reporter_controller);
            loop {
                send_agent_heartbeat(
                    &reporter,
                    &endpoint,
                    &reporter_token,
                    reporter_target_id,
                    &reporter_info,
                    &reporter_state,
                )
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
        if config.info.platform == AgentPlatform::Windows {
            tokio::spawn(workflow_client::run_windows_workflow_poller(
                workflow_controller,
                config.token.clone(),
                state.clone(),
                target_id,
                config.info.clone(),
            ));
        }
    }
    axum::serve(listener, agent_router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        ProcTelemetryInput, parse_agent_environment, run, startup_config_from_values,
        telemetry_from_proc, value_or_default,
    };

    #[test]
    fn environment_values_fall_back_only_when_missing() {
        assert_eq!(
            value_or_default(Some("configured".to_owned()), "fallback"),
            "configured"
        );
        assert_eq!(value_or_default(None, "fallback"), "fallback");
    }

    #[test]
    fn service_environment_accepts_only_unique_allowlisted_values() {
        let parsed = parse_agent_environment(
            "LXCUP_AGENT_TOKEN=secret\nLXCUP_AGENT_ID=target\nLXCUP_AGENT_BIND_ADDRESS=127.0.0.1:8090\n",
        )
        .unwrap();
        assert_eq!(
            parsed.get("LXCUP_AGENT_TOKEN").map(String::as_str),
            Some("secret")
        );
        assert!(parse_agent_environment("PATH=C:\\Windows").is_err());
        assert!(parse_agent_environment("LXCUP_AGENT_TOKEN=one\nLXCUP_AGENT_TOKEN=two").is_err());
        assert!(parse_agent_environment("LXCUP_AGENT_TOKEN=").is_err());
        assert!(parse_agent_environment("not-a-key-value").is_err());
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
        let (sample, _) = telemetry_from_proc(ProcTelemetryInput {
            meminfo: "MemTotal: 1000 kB\nMemAvailable: 250 kB\n",
            loadavg: "1.25 0.50 0.25 1/10 20",
            stat: "cpu 10 0 20 60 10 0 0 0",
            network: "Inter-| Receive | Transmit\n face |bytes packets\neth0: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n",
            storage_basis_points: Some(7_500),
            process_count: Some(42),
            previous_cpu: Some((0, 0)),
            collected_at: now,
        });
        assert_eq!(sample.collected_at, now);
        assert_eq!(sample.memory_basis_points, Some(7_500));
        assert_eq!(sample.load_1_milli, Some(1_250));
        assert_eq!(sample.cpu_basis_points, Some(3_000));
        assert_eq!(sample.storage_basis_points, Some(7_500));
        assert_eq!(sample.process_count, Some(42));
        assert_eq!(sample.network_rx_bytes, Some(100));
        assert_eq!(sample.network_tx_bytes, Some(200));

        let (malformed, _) = telemetry_from_proc(ProcTelemetryInput {
            meminfo: "MemTotal: nope",
            loadavg: "",
            stat: "cpu invalid",
            network: "",
            storage_basis_points: None,
            process_count: None,
            previous_cpu: None,
            collected_at: now,
        });
        assert_eq!(malformed.memory_basis_points, None);
        assert_eq!(malformed.load_1_milli, None);
        assert_eq!(malformed.cpu_basis_points, None);
        assert_eq!(malformed.storage_basis_points, None);
    }

    #[test]
    fn linux_cpu_usage_uses_interval_deltas_not_time_since_boot() {
        let now = chrono::Utc::now();
        let (sample, current) = telemetry_from_proc(ProcTelemetryInput {
            meminfo: "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
            loadavg: "0.00 0.00 0.00 1/1 1",
            stat: "cpu 20 0 20 130 0 0 0 0",
            network: "",
            storage_basis_points: None,
            process_count: None,
            previous_cpu: Some((100, 70)),
            collected_at: now,
        });
        assert_eq!(sample.cpu_basis_points, Some(1_428));
        assert_eq!(current, Some((170, 130)));
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
        let sample = super::collect_telemetry_with_cpu(None).await.0;
        assert!(sample.collected_at >= before);
        assert!(sample.collected_at <= chrono::Utc::now());
    }
}
