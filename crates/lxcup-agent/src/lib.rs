//! Versionierter HTTP-Vertrag zwischen lxcup und Linux-/Windows-Agenten.
//!
//! Agenten laufen innerhalb der verwalteten LXC/Windows-Systeme. Der Server
//! kennt nur diesen Vertrag und niemals lokale Shell-Details oder Secrets.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;
use uuid::Uuid;

mod parsers;
use parsers::{
    normalize_apt_list, parse_docker_containers, parse_dpkg_packages, parse_windows_packages,
    safe_detail, safe_package,
};

pub const PROTOCOL_VERSION: &str = "v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentPlatform {
    Linux,
    Windows,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub platform: AgentPlatform,
    pub hostname: String,
    pub version: String,
    pub protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAction {
    Scan,
    Apply,
    Health,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCommandRequest {
    pub action: AgentAction,
    #[serde(default)]
    pub packages: Vec<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCommandResponse {
    pub request_id: Uuid,
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub reboot_required: bool,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentMetrics {
    pub collected_at: DateTime<Utc>,
    pub commands_total: u64,
    pub commands_failed: u64,
    pub last_command_at: Option<DateTime<Utc>>,
}

/// A compact, platform-neutral sample. Percentages are basis points to avoid
/// floating point ambiguity on the wire; absent values explicitly mean a
/// platform collector was unavailable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemTelemetrySample {
    pub collected_at: DateTime<Utc>,
    pub cpu_basis_points: Option<u16>,
    pub memory_basis_points: Option<u16>,
    pub storage_basis_points: Option<u16>,
    pub load_1_milli: Option<u32>,
    pub network_rx_bytes: Option<u64>,
    pub network_tx_bytes: Option<u64>,
    pub process_count: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemTelemetryWindow {
    pub samples: Vec<SystemTelemetrySample>,
    pub partial: bool,
}

#[derive(Default)]
pub struct TelemetryBuffer {
    samples: VecDeque<SystemTelemetrySample>,
}

impl TelemetryBuffer {
    /// Retain twice the heartbeat interval so a later report can repair a
    /// missed delivery without leaving holes in the controller's timeline.
    pub const WINDOW_SECONDS: i64 = 60;
    pub const MAX_SAMPLES: usize = 32;
    pub fn record(&mut self, sample: SystemTelemetrySample) {
        self.samples.push_back(sample);
        while self.samples.len() > Self::MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.remove_expired();
    }
    pub fn window(&mut self) -> SystemTelemetryWindow {
        self.remove_expired();
        let samples = self.samples.iter().cloned().collect::<Vec<_>>();
        let partial = samples.iter().any(|sample| {
            sample.cpu_basis_points.is_none()
                || sample.memory_basis_points.is_none()
                || sample.storage_basis_points.is_none()
        });
        SystemTelemetryWindow { samples, partial }
    }
    fn remove_expired(&mut self) {
        let threshold = Utc::now() - chrono::Duration::seconds(Self::WINDOW_SECONDS);
        while self
            .samples
            .front()
            .is_some_and(|sample| sample.collected_at < threshold)
        {
            self.samples.pop_front();
        }
    }
}

/// Nicht-sensitive Docker-Inventardaten, die ein Linux-Agent melden darf.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<String>,
    pub started_at: Option<String>,
    pub labels: Vec<String>,
}

/// Stable, read-only discovery result. A host without Docker is not unhealthy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerDiscovery {
    pub available: bool,
    pub reason: Option<String>,
    pub collected_at: DateTime<Utc>,
    pub containers: Vec<DockerContainerInfo>,
}

/// Sanitized package inventory returned by an agent. It intentionally contains
/// only package metadata, never command lines, repository credentials or logs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentPackageInventory {
    pub collected_at: DateTime<Utc>,
    pub packages: Vec<AgentInstalledPackage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentInstalledPackage {
    pub name: String,
    pub installed_version: String,
    pub architecture: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentHealth {
    pub healthy: bool,
    pub info: AgentInfo,
    pub metrics: AgentMetrics,
}

/// Outbound status message sent by agents to the controller. The agent owns
/// the connection direction so private LXC and Windows networks need no
/// controller-to-agent ingress rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentHeartbeat {
    pub target_id: uuid::Uuid,
    pub info: AgentInfo,
    pub metrics: AgentMetrics,
    pub sent_at: DateTime<Utc>,
    pub telemetry: SystemTelemetryWindow,
}

#[derive(Clone)]
pub struct AgentClientConfig {
    pub base_url: String,
    pub token: String,
    pub timeout: Duration,
    pub max_retries: u8,
    pub backoff: Duration,
    root_certificate_pem: Option<Vec<u8>>,
    private_network_http: bool,
}

impl fmt::Debug for AgentClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentClientConfig")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .field("backoff", &self.backoff)
            .field("private_network_http", &self.private_network_http)
            .finish()
    }
}

impl AgentClientConfig {
    pub fn new(
        base_url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, AgentConfigError> {
        let base_url = base_url.into();
        if !(base_url.starts_with("https://")
            || base_url.starts_with("http://127.0.0.1")
            || base_url.starts_with("http://localhost"))
        {
            return Err(AgentConfigError::InsecureBaseUrl);
        }
        let token = token.into();
        if token.trim().is_empty() {
            return Err(AgentConfigError::EmptyToken);
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
            timeout: Duration::from_secs(20),
            max_retries: 2,
            backoff: Duration::from_millis(150),
            root_certificate_pem: None,
            private_network_http: false,
        })
    }

    /// Creates an HTTP client for a literal private-network agent address.
    /// DNS names must be resolved and pinned by the caller before use.
    pub fn new_for_private_network_http(
        base_url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, AgentConfigError> {
        let base_url = base_url.into();
        let parsed =
            reqwest::Url::parse(&base_url).map_err(|_| AgentConfigError::InsecureBaseUrl)?;
        let private_address = parsed
            .host_str()
            .and_then(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(is_private_network_address);
        if parsed.scheme() != "http"
            || !private_address
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AgentConfigError::InsecureBaseUrl);
        }
        let token = token.into();
        if token.trim().is_empty() {
            return Err(AgentConfigError::EmptyToken);
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
            timeout: Duration::from_secs(20),
            max_retries: 2,
            backoff: Duration::from_millis(150),
            root_certificate_pem: None,
            private_network_http: true,
        })
    }

    #[must_use]
    pub fn with_retry_policy(mut self, max_retries: u8, backoff: Duration) -> Self {
        self.max_retries = max_retries;
        self.backoff = backoff;
        self
    }

    /// Adds the private CA used by a managed HTTPS agent. Certificate
    /// validation remains enabled; this only extends the trusted roots.
    #[must_use]
    pub fn with_root_certificate_pem(mut self, certificate_pem: impl AsRef<[u8]>) -> Self {
        self.root_certificate_pem = Some(certificate_pem.as_ref().to_vec());
        self
    }
}

fn is_private_network_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_private() || address.is_loopback() || address.is_link_local()
        }
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentConfigError {
    #[error("agent URL must use HTTPS or an explicitly allowed local/private HTTP address")]
    InsecureBaseUrl,
    #[error("agent token must not be empty")]
    EmptyToken,
}

#[derive(Clone)]
pub struct AgentClient {
    http: Client,
    config: AgentClientConfig,
}

impl fmt::Debug for AgentClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AgentClient {
    pub fn new(config: AgentClientConfig) -> Result<Self, AgentError> {
        let mut builder = Client::builder().timeout(config.timeout);
        if config.private_network_http {
            builder = builder.no_proxy();
        }
        if let Some(certificate_pem) = config.root_certificate_pem.as_deref() {
            let certificate =
                reqwest::Certificate::from_pem(certificate_pem).map_err(AgentError::ClientBuild)?;
            builder = builder.add_root_certificate(certificate);
        }
        let http = builder.build().map_err(AgentError::ClientBuild)?;
        Ok(Self { http, config })
    }

    pub async fn health(&self) -> Result<AgentHealth, AgentError> {
        self.get("/health").await
    }
    pub async fn metrics(&self) -> Result<AgentMetrics, AgentError> {
        self.get("/metrics").await
    }

    pub async fn docker_containers(&self) -> Result<DockerDiscovery, AgentError> {
        self.get("/docker/containers").await
    }
    pub async fn package_inventory(&self) -> Result<AgentPackageInventory, AgentError> {
        self.get("/packages").await
    }

    pub async fn command(
        &self,
        request: &AgentCommandRequest,
    ) -> Result<AgentCommandResponse, AgentError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(AgentError::InvalidRequest("idempotency key is required"));
        }
        if request
            .packages
            .iter()
            .any(|package| !safe_package(package))
        {
            return Err(AgentError::InvalidRequest("package argument is not safe"));
        }
        self.post("/command", request).await
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, AgentError> {
        self.request(path, None::<&()>).await
    }

    async fn post<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, AgentError> {
        self.request(path, Some(body)).await
    }

    async fn request<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, AgentError> {
        let url = format!("{}{}", self.config.base_url, path);
        let mut attempt = 0_u8;
        loop {
            let request = match body {
                Some(body) => self.http.post(&url).json(body),
                None => self.http.get(&url),
            };
            match request.bearer_auth(&self.config.token).send().await {
                Ok(response)
                    if response.status() == StatusCode::UNAUTHORIZED
                        || response.status() == StatusCode::FORBIDDEN =>
                {
                    return Err(AgentError::Unauthorized);
                }
                Ok(response) if response.status().is_success() => {
                    return response.json().await.map_err(AgentError::Decode);
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    if !status.is_server_error() || attempt >= self.config.max_retries {
                        return Err(AgentError::Api {
                            status,
                            message: safe_detail(&body),
                        });
                    }
                }
                Err(error) => {
                    if attempt >= self.config.max_retries {
                        return Err(AgentError::Transport(error));
                    }
                }
            }
            attempt = attempt.saturating_add(1);
            tokio::time::sleep(self.config.backoff.saturating_mul(u32::from(attempt))).await;
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent HTTP client could not be built")]
    ClientBuild(#[source] reqwest::Error),
    #[error("agent transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("agent authentication failed")]
    Unauthorized,
    #[error("agent request was invalid: {0}")]
    InvalidRequest(&'static str),
    #[error("agent returned HTTP {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("agent returned invalid JSON")]
    Decode(#[source] reqwest::Error),
}

impl AgentError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Api { status, .. } => status.is_server_error(),
            _ => false,
        }
    }
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "agent_unauthorized",
            Self::InvalidRequest(_) => "agent_invalid_request",
            Self::Api { .. } => "agent_api_error",
            Self::Transport(_) => "agent_unreachable",
            Self::ClientBuild(_) => "agent_client_error",
            Self::Decode(_) => "agent_invalid_response",
        }
    }
}

#[derive(Clone)]
pub struct LocalAgentState {
    pub info: AgentInfo,
    token: Arc<str>,
    metrics: Arc<tokio::sync::Mutex<AgentMetrics>>,
    telemetry: Arc<tokio::sync::Mutex<TelemetryBuffer>>,
    results: Arc<tokio::sync::Mutex<HashMap<String, AgentCommandResponse>>>,
}

impl LocalAgentState {
    pub fn new(info: AgentInfo, token: impl Into<Arc<str>>) -> Self {
        Self {
            info,
            token: token.into(),
            metrics: Arc::new(tokio::sync::Mutex::new(AgentMetrics {
                collected_at: Utc::now(),
                commands_total: 0,
                commands_failed: 0,
                last_command_at: None,
            })),
            telemetry: Arc::new(tokio::sync::Mutex::new(TelemetryBuffer::default())),
            results: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub async fn metrics_snapshot(&self) -> AgentMetrics {
        self.metrics.lock().await.clone()
    }
    pub async fn record_telemetry(&self, sample: SystemTelemetrySample) {
        self.telemetry.lock().await.record(sample);
    }
    pub async fn telemetry_window(&self) -> SystemTelemetryWindow {
        self.telemetry.lock().await.window()
    }
}

pub fn agent_router(state: LocalAgentState) -> Router {
    Router::new()
        .route("/health", get(agent_health))
        .route("/metrics", get(agent_metrics))
        .route("/docker/containers", get(agent_docker_containers))
        .route("/packages", get(agent_package_inventory))
        .route("/command", post(agent_command))
        .with_state(state)
}

async fn agent_package_inventory(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    let mut command = if state.info.platform == AgentPlatform::Windows {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "Get-Package | Select-Object -Property Name,Version,ProviderName | ConvertTo-Json -Compress"]);
        command
    } else {
        let mut command = Command::new("dpkg-query");
        command.args([
            "-W",
            "-f=${binary:Package}\\t${Version}\\t${Architecture}\\n",
        ]);
        command
    };
    let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output.stdout,
        Ok(Ok(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"package_manager_unavailable"})),
            )
                .into_response();
        }
        Ok(Err(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"package_manager_unavailable"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(serde_json::json!({"error":"package_inventory_timeout"})),
            )
                .into_response();
        }
    };
    let packages = if state.info.platform == AgentPlatform::Windows {
        parse_windows_packages(&String::from_utf8_lossy(&output))
    } else {
        parse_dpkg_packages(&String::from_utf8_lossy(&output))
    };
    if packages.len() > 50_000 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error":"package_inventory_too_large"})),
        )
            .into_response();
    }
    Json(AgentPackageInventory {
        collected_at: Utc::now(),
        packages,
    })
    .into_response()
}

async fn agent_docker_containers(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
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
        _ => return Json(DockerDiscovery { available: false, reason: Some("docker_unavailable".to_owned()), collected_at: Utc::now(), containers: Vec::new() }).into_response(),
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

async fn agent_health(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    Json(AgentHealth {
        healthy: true,
        info: state.info.clone(),
        metrics: state.metrics.lock().await.clone(),
    })
    .into_response()
}

async fn agent_metrics(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    Json(state.metrics.lock().await.clone()).into_response()
}

async fn agent_command(
    State(state): State<LocalAgentState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<AgentCommandRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    if request.idempotency_key.trim().is_empty()
        || request
            .packages
            .iter()
            .any(|package| !safe_package(package))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"invalid_request"})),
        )
            .into_response();
    }
    if let Some(cached) = state
        .results
        .lock()
        .await
        .get(&request.idempotency_key)
        .cloned()
    {
        return Json(cached).into_response();
    }
    tracing::info!(agent_id = %state.info.agent_id, action = ?request.action, idempotency_key = %request.idempotency_key, "agent command requested");
    let started = std::time::Instant::now();
    let (exit_code, stdout, stderr) =
        run_local_command(state.info.platform, request.action, &request.packages).await;
    let success = exit_code == 0;
    let now = Utc::now();
    let mut metrics = state.metrics.lock().await;
    metrics.commands_total += 1;
    if !success {
        metrics.commands_failed += 1;
    }
    metrics.last_command_at = Some(now);
    metrics.collected_at = now;
    drop(metrics);
    let response = AgentCommandResponse {
        request_id: Uuid::new_v4(),
        success,
        exit_code,
        stdout: safe_detail(&stdout),
        stderr: safe_detail(&stderr),
        reboot_required: false,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    let mut results = state.results.lock().await;
    if results.len() >= 512 {
        results.clear();
    }
    results.insert(request.idempotency_key, response.clone());
    Json(response).into_response()
}

fn authorized(state: &LocalAgentState, headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == state.token.as_ref())
}

async fn run_local_command(
    platform: AgentPlatform,
    action: AgentAction,
    packages: &[String],
) -> (i32, String, String) {
    let mut command = if platform == AgentPlatform::Windows {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command"]);
        let script = match action {
            AgentAction::Health => "Write-Output 'healthy'",
            AgentAction::Scan => {
                "if (Get-Command Get-WindowsUpdate -ErrorAction SilentlyContinue) { Get-WindowsUpdate -MicrosoftUpdate -IgnoreReboot | ConvertTo-Json -Compress } else { Write-Error 'PSWindowsUpdate is not installed'; exit 2 }"
            }
            AgentAction::Apply => {
                "if (Get-Command Install-WindowsUpdate -ErrorAction SilentlyContinue) { Install-WindowsUpdate -AcceptAll -IgnoreReboot } else { Write-Error 'PSWindowsUpdate is not installed'; exit 2 }"
            }
        };
        command.arg(script);
        command
    } else {
        let mut command = Command::new("apt");
        match action {
            AgentAction::Health => {
                command.arg("--version");
            }
            AgentAction::Scan => {
                command.args(["list", "--upgradable"]);
            }
            AgentAction::Apply => {
                if packages.is_empty() {
                    return (
                        2,
                        String::new(),
                        "apply requires at least one package".to_owned(),
                    );
                }
                command.args(["--yes", "--only-upgrade", "install"]);
                command.args(packages);
            }
        }
        command
    };
    match tokio::time::timeout(Duration::from_secs(900), command.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stdout = if platform == AgentPlatform::Linux && action == AgentAction::Scan {
                normalize_apt_list(&stdout)
            } else {
                stdout
            };
            (
                output.status.code().unwrap_or(1),
                stdout,
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        }
        Ok(Err(error)) => (1, String::new(), error.to_string()),
        Err(_) => (124, String::new(), "agent command timed out".to_owned()),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
