use super::{ApiError, ApiState};
use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use lxcup_core::ActorRole;
use lxcup_persistence::{AuthAuditEvent, AuthUserRole};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
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
        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))?;
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
    let authenticated_actor =
        match authenticate_request(&state, &mut request, &path, request_id).await {
            Ok(actor) => actor,
            Err(response) => return response,
        };
    let method = request.method().clone();
    let _backup_read_guard = if is_write_method(&method) && path != "/api/v1/admin/backups" {
        Some(state.backup_gate.read().await)
    } else {
        None
    };
    request.extensions_mut().insert(request_id);
    let audit_id = match reserve_request_audit(
        &state,
        authenticated_actor.as_ref(),
        &method,
        &path,
        request_id,
    )
    .await
    {
        Ok(audit_id) => audit_id,
        Err(response) => return response,
    };
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let span = tracing::info_span!("http_request", request_id = %request_id, method = %request.method(), path = %path);
    let response = next.run(request).instrument(span).await;
    finalize_request_response(&state, response, &path, request_id, audit_id).await
}

async fn authenticate_request(
    state: &ApiState,
    request: &mut axum::extract::Request,
    path: &str,
    request_id: Uuid,
) -> Result<Option<super::super::AuthenticatedUser>, Response> {
    let public = is_public_request(state, path);
    if state.account_auth_enabled && !public {
        let (actor, cookie_authenticated) = load_account_actor(state, request.headers()).await?;
        let role = authorize_account_request(
            state,
            &actor,
            cookie_authenticated,
            request.method().clone(),
            request.headers().clone(),
            path,
            request_id,
        )
        .await?;
        request.extensions_mut().insert(role);
        request.extensions_mut().insert(actor.clone());
        return Ok(Some(actor));
    }

    let role = state.auth.role(request.headers());
    authorize_legacy_role(&state.auth, role, public, path, request.method())?;
    if let Some(role) = role {
        request.extensions_mut().insert(role);
    }
    Ok(None)
}

fn is_public_request(state: &ApiState, path: &str) -> bool {
    // The heartbeat handler validates its own per-target agent token.
    matches!(
        path,
        "/health/live" | "/health/ready" | "/metrics" | "/api/v1/agents/heartbeat"
    ) || (state.account_auth_enabled
        && matches!(path, "/api/v1/auth/status" | "/api/v1/auth/login"))
}

async fn load_account_actor(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<(super::super::AuthenticatedUser, bool), Response> {
    let Some((token, cookie_authenticated)) = session_credential(headers) else {
        return Err(ApiError::unauthorized().into_response());
    };
    let Some(repositories) = state.repositories.as_ref() else {
        return Err(ApiError::storage().into_response());
    };
    let token_hash = super::super::hash_session_token(token);
    let session = match repositories.auth.session_by_token_hash(&token_hash).await {
        Ok(session) => session,
        Err(_) => return Err(ApiError::storage().into_response()),
    };
    let Some(session) = session else {
        return Err(ApiError::unauthorized().into_response());
    };
    let user = session.user;
    Ok((
        super::super::AuthenticatedUser {
            id: user.id,
            username: user.username,
            role: user.role,
            must_change_password: user.must_change_password,
            token_hash,
            expires_at: session.expires_at,
        },
        cookie_authenticated,
    ))
}

async fn authorize_account_request(
    state: &ApiState,
    actor: &super::super::AuthenticatedUser,
    cookie_authenticated: bool,
    method: Method,
    headers: HeaderMap,
    path: &str,
    request_id: Uuid,
) -> Result<ActorRole, Response> {
    if cookie_authenticated && is_unsafe_method(&method) && !valid_csrf_request(&headers) {
        return Err(audited_forbidden_response(
            state,
            actor,
            &method,
            path,
            request_id,
            "csrf_validation_failed",
            "the request could not be verified",
        )
        .await);
    }
    if actor.must_change_password && !is_password_change_allowed_path(path) {
        return Err(ApiError::forbidden(
            "password_change_required",
            "change the initial password before using the application",
        )
        .into_response());
    }

    let role = actor_role(actor.role);
    if is_admin_only_route(path) && actor.role != AuthUserRole::Admin {
        return Err(audited_forbidden_response(
            state,
            actor,
            &method,
            path,
            request_id,
            "permission_denied",
            "the current role cannot perform this action",
        )
        .await);
    }
    if !state.auth.allows(path, &method, &role) {
        return Err(audited_forbidden_response(
            state,
            actor,
            &method,
            path,
            request_id,
            "permission_denied",
            "the current role cannot perform this action",
        )
        .await);
    }
    Ok(role)
}

fn is_password_change_allowed_path(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/auth/session" | "/api/v1/auth/logout" | "/api/v1/auth/password"
    )
}

fn actor_role(role: AuthUserRole) -> ActorRole {
    match role {
        AuthUserRole::Admin => ActorRole::Admin,
        AuthUserRole::User => ActorRole::Operator,
    }
}

fn authorize_legacy_role(
    auth: &AuthConfig,
    role: Option<ActorRole>,
    public: bool,
    path: &str,
    method: &Method,
) -> Result<(), Response> {
    if !public && role.is_none() {
        return Err(ApiError::unauthorized().into_response());
    }
    if role.is_some_and(|role| !public && !auth.allows(path, method, &role)) {
        return Err(ApiError::forbidden(
            "permission_denied",
            "the current role cannot perform this action",
        )
        .into_response());
    }
    Ok(())
}

async fn audited_forbidden_response(
    state: &ApiState,
    actor: &super::super::AuthenticatedUser,
    method: &Method,
    path: &str,
    request_id: Uuid,
    code: &'static str,
    message: &'static str,
) -> Response {
    append_request_audit(
        state,
        actor,
        method,
        path,
        request_id,
        StatusCode::FORBIDDEN,
    )
    .await;
    ApiError::forbidden(code, message).into_response()
}

async fn reserve_request_audit(
    state: &ApiState,
    actor: Option<&super::super::AuthenticatedUser>,
    method: &Method,
    path: &str,
    request_id: Uuid,
) -> Result<Option<Uuid>, Response> {
    let Some(actor) = actor.filter(|_| {
        matches!(
            *method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        )
    }) else {
        return Ok(None);
    };
    let event = request_audit_event(actor, method, path, request_id, StatusCode::PROCESSING);
    let Some(repositories) = state.repositories.as_ref() else {
        return Err(ApiError::storage().into_response());
    };
    if let Err(error) = repositories.auth.append_audit_event(&event).await {
        tracing::error!(error = ?error, request_id = %request_id, "request audit reservation could not be persisted");
        return Err(ApiError::storage().into_response());
    }
    Ok(Some(event.id))
}

async fn finalize_request_response(
    state: &ApiState,
    mut response: Response,
    path: &str,
    request_id: Uuid,
    audit_id: Option<Uuid>,
) -> Response {
    if let Ok(value) = request_id.to_string().parse() {
        response.headers_mut().insert("x-request-id", value);
    }
    if should_clear_logout_cookies(path, &response) {
        let headers = match crate::user_auth::cleared_session_cookie_headers(state.secure_cookies) {
            Ok(headers) => headers,
            Err(_) => return ApiError::storage().into_response(),
        };
        for value in headers.get_all(header::SET_COOKIE).iter() {
            response
                .headers_mut()
                .append(header::SET_COOKIE, value.clone());
        }
    }
    if response.status().is_client_error() || response.status().is_server_error() {
        state
            .metrics
            .requests_failed
            .fetch_add(1, Ordering::Relaxed);
    }
    persist_request_audit_outcome(state, audit_id, response.status(), request_id).await;
    response
}

fn should_clear_logout_cookies(path: &str, response: &Response) -> bool {
    path == "/api/v1/auth/logout" && response.status().is_success()
}

async fn persist_request_audit_outcome(
    state: &ApiState,
    audit_id: Option<Uuid>,
    status: StatusCode,
    request_id: Uuid,
) {
    let Some(audit_id) = audit_id else {
        return;
    };
    let Some(repositories) = state.repositories.as_ref() else {
        return;
    };
    let outcome = if status.is_success() {
        "completed"
    } else {
        "failed"
    };
    match repositories
        .auth
        .update_audit_event_status(audit_id, status.as_u16() as i16, outcome)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::error!(request_id = %request_id, "request audit reservation disappeared before completion")
        }
        Err(error) => {
            tracing::error!(error = ?error, request_id = %request_id, "request audit outcome could not be persisted; reservation remains pending")
        }
    }
}

async fn append_request_audit(
    state: &ApiState,
    actor: &super::super::AuthenticatedUser,
    method: &Method,
    path: &str,
    request_id: Uuid,
    status: StatusCode,
) {
    let Some(repositories) = state.repositories.as_ref() else {
        return;
    };
    let event = request_audit_event(actor, method, path, request_id, status);
    if let Err(error) = repositories.auth.append_audit_event(&event).await {
        tracing::error!(error = ?error, "user activity audit event could not be persisted");
    }
}

fn request_audit_event(
    actor: &super::super::AuthenticatedUser,
    method: &Method,
    path: &str,
    request_id: Uuid,
    status: StatusCode,
) -> AuthAuditEvent {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let _ = segments.next();
    let _ = segments.next();
    let resource_type = segments.next().unwrap_or("api");
    let resource_id = segments.next().filter(|value| !is_action_segment(value));
    AuthAuditEvent {
        id: Uuid::new_v4(),
        actor_user_id: Some(actor.id),
        actor_username: actor.username.clone(),
        actor_role: actor.role,
        action: audit_action(method, path),
        resource_type: resource_type.to_owned(),
        resource_id: resource_id.map(str::to_owned),
        request_id: Some(request_id),
        status_code: status.as_u16() as i16,
        details: json!({
            "path": path,
            "state": if status == StatusCode::PROCESSING {
                "pending"
            } else if status.is_success() {
                "completed"
            } else {
                "failed"
            }
        }),
        created_at: Utc::now(),
    }
}

fn audit_action(method: &Method, path: &str) -> String {
    let parts = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["api", "v1", "auth", "logout"]) => "auth.logout".to_owned(),
        ("POST", ["api", "v1", "auth", "password"]) => "auth.password_changed".to_owned(),
        ("POST", ["api", "v1", "admin", "backups"]) => "admin.backup_created".to_owned(),
        ("POST", ["api", "v1", "targets"]) => "target.created".to_owned(),
        ("DELETE", ["api", "v1", "targets", _]) => "target.deleted".to_owned(),
        ("POST", ["api", "v1", "targets", _, "docker", "discovery"]) => {
            "docker.discovery_requested".to_owned()
        }
        (
            "POST",
            [
                "api",
                "v1",
                "targets",
                _,
                "docker",
                "containers",
                _,
                "action",
            ],
        ) => "docker.lifecycle_action".to_owned(),
        (
            "POST",
            [
                "api",
                "v1",
                "targets",
                _,
                "docker",
                "containers",
                _,
                "image-update-check",
            ],
        ) => "docker.image_update_checked".to_owned(),
        (
            "POST",
            [
                "api",
                "v1",
                "targets",
                _,
                "docker",
                "containers",
                _,
                "image-update-apply",
            ],
        ) => "docker.image_update_applied".to_owned(),
        ("POST", ["api", "v1", "secrets"]) => "secret.created".to_owned(),
        ("DELETE", ["api", "v1", "secrets", _]) => "secret.deleted".to_owned(),
        ("POST", ["api", "v1", "secrets", _, "rotate"]) => "secret.rotated".to_owned(),
        ("POST", ["api", "v1", "secrets", _, "revoke"]) => "secret.revoked".to_owned(),
        ("POST", ["api", "v1", "ansible", "jobs"]) => "workflow.queued".to_owned(),
        ("POST", ["api", "v1", "ansible", "jobs", _, "retry"]) => "workflow.retried".to_owned(),
        ("POST", ["api", "v1", "ansible", "jobs", _, "reconcile"]) => {
            "workflow.reconciled".to_owned()
        }
        ("POST", ["api", "v1", "schedules"]) => "schedule.created".to_owned(),
        ("PATCH", ["api", "v1", "schedules", _]) => "schedule.updated".to_owned(),
        ("POST", ["api", "v1", "update-policies"]) => "update_policy.created".to_owned(),
        ("DELETE", ["api", "v1", "update-policies", _]) => "update_policy.deleted".to_owned(),
        ("POST", ["api", "v1", "enrollments"]) => "enrollment.created".to_owned(),
        ("POST", ["api", "v1", "containers", _, "scans"]) => "scan.started".to_owned(),
        ("POST", ["api", "v1", "scans", _, "run"]) => "scan.executed".to_owned(),
        ("POST", ["api", "v1", "containers", _, "plans"]) => "plan.created".to_owned(),
        ("POST", ["api", "v1", "plans", _, "confirm"]) => "plan.confirmed".to_owned(),
        ("POST", ["api", "v1", "executions", _, "abort"]) => "execution.aborted".to_owned(),
        ("POST", ["api", "v1", "executions", _, "run"]) => "execution.started".to_owned(),
        ("POST", ["api", "v1", "executions", _, "reconcile"]) => "execution.reconciled".to_owned(),
        ("POST", ["api", "v1", "users"]) => "user.created".to_owned(),
        ("PATCH", ["api", "v1", "users", _]) => "user.updated".to_owned(),
        ("DELETE", ["api", "v1", "users", _]) => "user.deleted".to_owned(),
        ("POST", ["api", "v1", "users", _, "password-reset"]) => "user.password_reset".to_owned(),
        ("POST", ["api", "v1", "containers", _, "agent"]) => "agent.registered".to_owned(),
        ("POST", ["api", "v1", "containers", _, "agent", "revoke"]) => "agent.revoked".to_owned(),
        ("POST", ["api", "v1", "containers", _, "docker", "discover"]) => {
            "docker.discovery_requested".to_owned()
        }
        (
            "POST",
            [
                "api",
                "v1",
                "containers",
                _,
                "docker",
                "containers",
                _,
                "adopt",
            ],
        ) => "docker.container_adopted".to_owned(),
        ("DELETE", ["api", "v1", "containers", _, "docker", "containers", _]) => {
            "docker.container_removed".to_owned()
        }
        _ => format!("http.{}", method.as_str().to_ascii_lowercase()),
    }
}

fn is_action_segment(segment: &str) -> bool {
    matches!(
        segment,
        "password"
            | "password-reset"
            | "revoke"
            | "rotate"
            | "disable"
            | "abort"
            | "retry"
            | "reconcile"
            | "action"
            | "image-update-check"
            | "image-update-apply"
            | "discovery"
            | "discover"
            | "events"
            | "audit"
            | "confirm"
            | "run"
    )
}

fn session_credential(headers: &HeaderMap) -> Option<(&str, bool)> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        return value
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
            .map(|token| (token, false));
    }
    cookie_value(headers, crate::user_auth::SESSION_COOKIE_NAME).map(|token| (token, true))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut result = None;
    for header_value in headers.get_all(header::COOKIE).iter() {
        let cookies = header_value.to_str().ok()?;
        for cookie in cookies.split(';') {
            let Some((cookie_name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if cookie_name == name {
                if result.is_some() {
                    return None;
                }
                result = Some(value.trim());
            }
        }
    }
    result
}

fn is_unsafe_method(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn valid_csrf_request(headers: &HeaderMap) -> bool {
    let (Some(cookie_token), Some(header_token)) = (
        cookie_value(headers, crate::user_auth::CSRF_COOKIE_NAME),
        headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok()),
    ) else {
        return false;
    };
    cookie_token.len() == header_token.len()
        && bool::from(cookie_token.as_bytes().ct_eq(header_token.as_bytes()))
}

fn is_admin_only_route(path: &str) -> bool {
    path.starts_with("/api/v1/users")
        || path.starts_with("/api/v1/secrets")
        || path == "/api/v1/auth/audit"
        || path.starts_with("/api/v1/admin/")
}

fn is_write_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

#[path = "health.rs"]
mod health;
pub(crate) use health::{live_health, metrics, ready_health};

#[cfg(test)]
#[path = "auth_tests.rs"]
mod user_route_tests;
