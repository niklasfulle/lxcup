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

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

mod docker;
mod package_inventory;
mod parsers;
pub use package_inventory::{PackageInventoryError, collect_package_inventory};
pub use parsers::{
    WindowsPackageParseError, is_safe_docker_container_id, parse_docker_stats,
    parse_remote_image_config_digest, parse_windows_packages, safe_winget_id,
};
use parsers::{normalize_apt_list, parse_dpkg_packages, safe_detail, safe_package};

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

/// A bounded allowlisted task claimed by the installed Windows agent over
/// its authenticated outbound connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentWorkflowCommand {
    pub job_id: uuid::Uuid,
    pub action: AgentAction,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentWorkflowClaimRequest {
    pub target_id: uuid::Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentWorkflowResult {
    pub response: AgentCommandResponse,
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

/// Resource samples reported for Docker containers running on this agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerTelemetrySample {
    pub collected_at: DateTime<Utc>,
    pub container_id: String,
    pub cpu_basis_points: Option<u16>,
    pub memory_basis_points: Option<u16>,
    pub memory_used_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerTelemetryWindow {
    pub samples: Vec<DockerTelemetrySample>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerLifecycleAction {
    Start,
    Stop,
    Restart,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerLifecycleRequest {
    pub container_id: String,
    pub action: DockerLifecycleAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerLifecycleResult {
    pub container_id: String,
    pub action: DockerLifecycleAction,
    pub completed_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerImageUpdateStatus {
    Current,
    UpdateAvailable,
    Pinned,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerImageUpdateRequest {
    pub container_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerImageUpdateResult {
    pub container_id: String,
    pub image: String,
    pub current_image_id: Option<String>,
    pub remote_image_id: Option<String>,
    pub status: DockerImageUpdateStatus,
    pub reason: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerImageUpdateApplyRequest {
    pub container_id: String,
    pub expected_remote_image_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerImageUpdateApplyResult {
    pub container_id: String,
    pub image: String,
    pub compose_project: String,
    pub compose_service: String,
    pub image_id: String,
    pub service_state: String,
    pub health_status: Option<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct DockerTelemetryBuffer {
    samples: VecDeque<DockerTelemetrySample>,
}

impl DockerTelemetryBuffer {
    pub const WINDOW_SECONDS: i64 = 60;
    pub const MAX_SAMPLES: usize = 8_000;

    pub fn record(&mut self, samples: impl IntoIterator<Item = DockerTelemetrySample>) {
        self.samples.extend(samples);
        while self.samples.len() > Self::MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.remove_expired();
    }

    pub fn window(&mut self) -> DockerTelemetryWindow {
        self.remove_expired();
        DockerTelemetryWindow {
            samples: self.samples.iter().cloned().collect(),
        }
    }

    fn remove_expired(&mut self) {
        let threshold = Utc::now() - chrono::Duration::seconds(Self::WINDOW_SECONDS);
        self.samples
            .retain(|sample| sample.collected_at >= threshold);
    }
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
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub image_id: Option<String>,
    #[serde(default)]
    pub restart_count: Option<u64>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub oom_killed: Option<bool>,
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
    #[serde(default)]
    pub candidate_version: Option<String>,
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
    #[serde(default)]
    pub docker_telemetry: DockerTelemetryWindow,
    #[serde(default)]
    pub package_inventory: Option<AgentPackageInventory>,
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
    pub async fn docker_lifecycle(
        &self,
        request: &DockerLifecycleRequest,
    ) -> Result<DockerLifecycleResult, AgentError> {
        if !is_safe_docker_container_id(&request.container_id) {
            return Err(AgentError::InvalidRequest("Docker container id is invalid"));
        }
        self.post("/docker/containers/action", request).await
    }
    pub async fn docker_image_update_check(
        &self,
        request: &DockerImageUpdateRequest,
    ) -> Result<DockerImageUpdateResult, AgentError> {
        if !is_safe_docker_container_id(&request.container_id) {
            return Err(AgentError::InvalidRequest("Docker container id is invalid"));
        }
        self.post("/docker/containers/image-update-check", request)
            .await
    }
    pub async fn docker_image_update_apply(
        &self,
        request: &DockerImageUpdateApplyRequest,
    ) -> Result<DockerImageUpdateApplyResult, AgentError> {
        if !is_safe_docker_container_id(&request.container_id)
            || !request
                .expected_remote_image_id
                .strip_prefix("sha256:")
                .is_some_and(|digest| {
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        {
            return Err(AgentError::InvalidRequest(
                "Docker image update request is invalid",
            ));
        }
        self.post("/docker/containers/image-update-apply", request)
            .await
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
    docker_telemetry: Arc<tokio::sync::Mutex<DockerTelemetryBuffer>>,
    package_inventory: Arc<tokio::sync::Mutex<Option<AgentPackageInventory>>>,
    results: Arc<tokio::sync::Mutex<HashMap<String, AgentCommandResponse>>>,
    docker_command_runner: Arc<dyn docker::DockerCommandRunner>,
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
            docker_telemetry: Arc::new(tokio::sync::Mutex::new(DockerTelemetryBuffer::default())),
            package_inventory: Arc::new(tokio::sync::Mutex::new(None)),
            results: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            docker_command_runner: Arc::new(docker::ProcessDockerCommandRunner),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_docker_command_runner(
        mut self,
        runner: Arc<dyn docker::DockerCommandRunner>,
    ) -> Self {
        self.docker_command_runner = runner;
        self
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
    pub async fn record_docker_telemetry(
        &self,
        samples: impl IntoIterator<Item = DockerTelemetrySample>,
    ) {
        self.docker_telemetry.lock().await.record(samples);
    }
    pub async fn docker_telemetry_window(&self) -> DockerTelemetryWindow {
        self.docker_telemetry.lock().await.window()
    }
    pub async fn record_package_inventory(&self, inventory: AgentPackageInventory) {
        *self.package_inventory.lock().await = Some(inventory);
    }
    pub async fn package_inventory_snapshot(&self) -> Option<AgentPackageInventory> {
        self.package_inventory.lock().await.clone()
    }
    pub async fn acknowledge_package_inventory(&self, collected_at: DateTime<Utc>) {
        let mut inventory = self.package_inventory.lock().await;
        if inventory
            .as_ref()
            .is_some_and(|current| current.collected_at == collected_at)
        {
            *inventory = None;
        }
    }
}

mod agent_routes;
pub use agent_routes::agent_router;
pub(crate) use agent_routes::authorized;
pub use agent_routes::execute_workflow_command;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
