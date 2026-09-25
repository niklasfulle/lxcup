use super::{ApiError, ApiState};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use lxcup_core::ActorRole;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::Instrument;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    viewer_token: Option<Arc<str>>,
    operator_token: Option<Arc<str>>,
    admin_token: Option<Arc<str>>,
    required: bool,
    token_expires_at: Option<Instant>,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let viewer_token = std::env::var("LXCUP_AUTH_VIEWER_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let operator_token = std::env::var("LXCUP_AUTH_OPERATOR_TOKEN")
            .ok()
            .or_else(|| std::env::var("LXCUP_AUTH_TOKEN").ok())
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let admin_token = std::env::var("LXCUP_AUTH_ADMIN_TOKEN")
            .ok()
            .or_else(|| std::env::var("LXCUP_AUTH_TOKEN").ok())
            .filter(|value| !value.trim().is_empty())
            .map(Arc::<str>::from);
        let required = std::env::var("LXCUP_AUTH_REQUIRED")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(operator_token.is_some() || viewer_token.is_some());
        let token_ttl = std::env::var("LXCUP_AUTH_TOKEN_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(8 * 60 * 60));
        Self {
            viewer_token,
            operator_token,
            admin_token,
            required,
            token_expires_at: Some(Instant::now() + token_ttl),
        }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    /// Production must have an explicit, non-empty token for every role.
    /// Development keeps the deliberately permissive default for local UI work.
    pub fn is_production_ready(&self) -> bool {
        let Some(viewer) = self.viewer_token.as_deref() else {
            return false;
        };
        let Some(operator) = self.operator_token.as_deref() else {
            return false;
        };
        let Some(admin) = self.admin_token.as_deref() else {
            return false;
        };
        self.required
            && viewer.len() >= 16
            && operator.len() >= 16
            && admin.len() >= 16
            && viewer != operator
            && viewer != admin
            && operator != admin
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn with_tokens(
        mut self,
        viewer: Option<String>,
        operator: Option<String>,
        admin: Option<String>,
    ) -> Self {
        self.viewer_token = viewer.map(Arc::<str>::from);
        self.operator_token = operator.map(Arc::<str>::from);
        self.admin_token = admin.map(Arc::<str>::from);
        self
    }

    /// Sets the lifetime of the currently loaded role tokens. Rotation or a
    /// fresh process creates a new lifetime; expired tokens fail closed.
    pub fn with_token_ttl(mut self, ttl: Duration) -> Self {
        self.token_expires_at = Some(Instant::now() + ttl);
        self
    }

    pub(crate) fn remaining_ttl_seconds(&self) -> Option<u64> {
        self.token_expires_at.map(|expires_at| {
            expires_at
                .saturating_duration_since(Instant::now())
                .as_secs()
        })
    }

    pub(super) fn allows(&self, path: &str, method: &Method, role: &ActorRole) -> bool {
        if path == "/api/v1/auth/logout" {
            return true;
        }
        if method == Method::GET || method == Method::HEAD {
            return role.grants(lxcup_core::Permission::Read);
        }
        // Workflow permissions depend on the registered operation and are
        // checked by the handler after parsing the request body.
        if (path == "/api/v1/ansible/jobs"
            || path.ends_with("/scans")
            || path.starts_with("/api/v1/scans/"))
            && method == Method::POST
        {
            return true;
        }
        let permission = if method == Method::DELETE
            || path.ends_with("/revoke")
            || path.ends_with("/disable")
            || path.ends_with("/abort")
            || (path.starts_with("/api/v1/secrets") && method != Method::GET)
        {
            lxcup_core::Permission::Destructive
        } else {
            lxcup_core::Permission::Configure
        };
        role.grants(permission)
    }

    pub(super) fn role(&self, headers: &HeaderMap) -> Option<ActorRole> {
        if !self.required {
            return Some(ActorRole::Admin);
        }
        if self
            .token_expires_at
            .is_some_and(|expires_at| Instant::now() >= expires_at)
        {
            return None;
        }
        let Some(token) = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return None;
        };
        if self.admin_token.as_deref() == Some(token) {
            Some(ActorRole::Admin)
        } else if self.operator_token.as_deref() == Some(token) {
            Some(ActorRole::Operator)
        } else if self.viewer_token.as_deref() == Some(token) {
            Some(ActorRole::Viewer)
        } else {
            None
        }
    }
}

pub(crate) struct ApiMetrics {
    pub(crate) started_at: Option<Instant>,
    pub(crate) requests_total: AtomicU64,
    pub(crate) requests_failed: AtomicU64,
    pub(crate) events_published: AtomicU64,
}

impl Default for ApiMetrics {
    fn default() -> Self {
        Self {
            started_at: Some(Instant::now()),
            requests_total: AtomicU64::default(),
            requests_failed: AtomicU64::default(),
            events_published: AtomicU64::default(),
        }
    }
}

impl ApiMetrics {
    fn started_at(&self) -> Instant {
        self.started_at.unwrap_or_else(Instant::now)
    }

    pub(super) fn render(&self) -> String {
        format!(
            "# TYPE lxcup_http_requests_total counter\nlxcup_http_requests_total {}\n# TYPE lxcup_http_requests_failed_total counter\nlxcup_http_requests_failed_total {}\n# TYPE lxcup_events_published_total counter\nlxcup_events_published_total {}\nlxcup_uptime_seconds {}\n",
            self.requests_total.load(Ordering::Relaxed),
            self.requests_failed.load(Ordering::Relaxed),
            self.events_published.load(Ordering::Relaxed),
            self.started_at().elapsed().as_secs()
        )
    }
}

pub(crate) async fn request_middleware(
    State(state): State<ApiState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4);
    // The heartbeat handler validates its own per-target agent token.
    let public = path == "/health/live"
        || path == "/health/ready"
        || path == "/metrics"
        || path == "/api/v1/agents/heartbeat";
    let role = state.auth.role(request.headers());
    if !public && role.is_none() {
        return ApiError::unauthorized().into_response();
    }
    if let Some(role) = role {
        if !public && !state.auth.allows(&path, request.method(), &role) {
            return ApiError::forbidden(
                "permission_denied",
                "the current role cannot perform this action",
            )
            .into_response();
        }
        request.extensions_mut().insert(role);
    }
    request.extensions_mut().insert(request_id);
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let span = tracing::info_span!("http_request", request_id = %request_id, method = %request.method(), path = %path);
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = request_id.to_string().parse() {
        response.headers_mut().insert("x-request-id", value);
    }
    if response.status().is_client_error() || response.status().is_server_error() {
        state
            .metrics
            .requests_failed
            .fetch_add(1, Ordering::Relaxed);
    }
    response
}

pub(crate) async fn live_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub(crate) async fn ready_health(State(state): State<ApiState>) -> impl IntoResponse {
    let targets_ready = state
        .store
        .read()
        .await
        .targets
        .iter()
        .all(|target| target.state != lxcup_core::TargetState::Disabled);
    let database_ready = match state.repositories.as_ref() {
        Some(repositories) => repositories.ansible_jobs.ping().await.is_ok(),
        None => true,
    };
    let ready = targets_ready && database_ready;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": if ready { "ready" } else { "degraded" },
            "checks": {
                "database": if database_ready { "ready" } else { "not_ready" },
                "targets": if targets_ready { "ready" } else { "degraded" },
            }
        })),
    )
}

pub(crate) async fn metrics(State(state): State<ApiState>) -> impl IntoResponse {
    let mut rendered = state.metrics.render();
    let (queue, last_seen) = if let Some(repositories) = state.repositories.as_ref() {
        let (queue, heartbeat) = tokio::join!(
            repositories.ansible_jobs.queue_metrics(),
            repositories.worker_heartbeats.latest()
        );
        (queue.unwrap_or_default(), heartbeat.ok().flatten())
    } else {
        (Default::default(), None)
    };
    let queue_age = queue
        .oldest_queued_at
        .map(|created| (chrono::Utc::now() - created).num_seconds().max(0))
        .unwrap_or(0);
    let heartbeat_age = last_seen
        .map(|seen| (chrono::Utc::now() - seen).num_seconds().max(0))
        .unwrap_or(-1);
    let worker_available = i32::from((0..=10).contains(&heartbeat_age));
    rendered.push_str(&format!(
        "# TYPE lxcup_ansible_jobs_queued gauge\nlxcup_ansible_jobs_queued {}\n# TYPE lxcup_ansible_jobs_failed gauge\nlxcup_ansible_jobs_failed {}\n# TYPE lxcup_ansible_queue_age_seconds gauge\nlxcup_ansible_queue_age_seconds {}\n# TYPE lxcup_worker_heartbeat_age_seconds gauge\nlxcup_worker_heartbeat_age_seconds {}\n# TYPE lxcup_worker_available gauge\nlxcup_worker_available {}\n",
        queue.queued_jobs, queue.failed_jobs, queue_age, heartbeat_age, worker_available
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        rendered,
    )
}
