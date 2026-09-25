use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, ContainerDto, ExecutionDto, ExecutionResultDto,
    PlanDto, Scan, ScanDto, envelope, parse_container_id, parse_uuid, truncate,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use lxcup_agent::{AgentAction, AgentCommandRequest};
use lxcup_core::{ActorRole, ScanId};

mod auth;
pub use auth::AuthConfig;
pub(super) use auth::*;
mod execution;
pub(super) use execution::*;

pub(super) async fn list_containers(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<ContainerDto>>> {
    let containers = state
        .store
        .read()
        .await
        .containers
        .iter()
        .map(ContainerDto::from)
        .collect();
    Json(envelope(containers))
}

/// Stateless bearer tokens are discarded by the client on logout. The
/// endpoint gives clients a stable audit-safe contract without echoing the
/// token; immediate server-side invalidation is provided by rotation/revoke.
pub(super) async fn logout(axum::Extension(_actor_role): axum::Extension<ActorRole>) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[derive(serde::Serialize)]
pub(super) struct AuthSessionDto {
    role: ActorRole,
    expires_in_seconds: Option<u64>,
}

pub(super) async fn auth_session(
    State(state): State<ApiState>,
    axum::Extension(role): axum::Extension<ActorRole>,
) -> Json<ApiEnvelope<AuthSessionDto>> {
    Json(envelope(AuthSessionDto {
        role,
        expires_in_seconds: state.auth.remaining_ttl_seconds(),
    }))
}

pub(super) async fn list_scans(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<ScanDto>>>, ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let scans = state
        .store
        .read()
        .await
        .scans
        .iter()
        .filter(|scan| scan.container_id == container_id)
        .map(ScanDto::from)
        .collect();
    Ok(Json(envelope(scans)))
}

pub(super) async fn start_scan(
    State(state): State<ApiState>,
    Path(container_id): Path<String>,
) -> Result<(StatusCode, Json<ApiEnvelope<ScanDto>>), ApiError> {
    let container_id = parse_container_id(&container_id)?;
    let mut store = state.store.write().await;
    if !store
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }
    let scan = Scan::new(container_id);
    let dto = ScanDto::from(&scan);
    store.scans.push(scan.clone());
    let repositories = state.repositories.clone();
    drop(store);
    if let Some(repositories) = repositories {
        repositories
            .scans
            .save(&scan)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(ApiEvent::status(
        "scan",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, Method};
    use std::{sync::atomic::Ordering, time::Duration};

    #[test]
    fn auth_config_distinguishes_viewer_read_access_from_mutations() {
        let config = AuthConfig::disabled().required(true).with_tokens(
            Some("viewer".to_owned()),
            Some("operator".to_owned()),
            Some("admin".to_owned()),
        );
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer viewer".parse().unwrap());
        assert_eq!(config.role(&headers), Some(ActorRole::Viewer));
        assert!(config.allows("/api/v1/targets", &Method::GET, &ActorRole::Viewer));
        assert!(!config.allows("/api/v1/targets", &Method::POST, &ActorRole::Viewer));
        assert!(config.allows("/api/v1/auth/logout", &Method::POST, &ActorRole::Viewer));
        assert!(config.allows("/api/v1/ansible/jobs", &Method::POST, &ActorRole::Viewer));
        headers.insert("authorization", "Bearer operator".parse().unwrap());
        assert_eq!(config.role(&headers), Some(ActorRole::Operator));
        assert!(config.allows("/api/v1/targets", &Method::POST, &ActorRole::Operator));
        assert!(!config.allows("/api/v1/secrets", &Method::POST, &ActorRole::Operator));
        assert!(!config.allows("/api/v1/targets/id", &Method::DELETE, &ActorRole::Operator));
        headers.insert("authorization", "Bearer admin".parse().unwrap());
        assert_eq!(config.role(&headers), Some(ActorRole::Admin));
        assert!(config.allows("/api/v1/secrets/id", &Method::DELETE, &ActorRole::Admin));
        headers.insert("authorization", "Bearer unknown".parse().unwrap());
        assert_eq!(config.role(&headers), None);
    }

    #[test]
    fn production_auth_requires_distinct_long_lived_role_tokens() {
        let valid = AuthConfig::disabled().required(true).with_tokens(
            Some("viewer-token-123456".to_owned()),
            Some("operator-token-123456".to_owned()),
            Some("admin-token-123456".to_owned()),
        );
        assert!(valid.is_production_ready());
        assert!(!AuthConfig::disabled().is_production_ready());
        assert!(
            !AuthConfig::disabled()
                .required(true)
                .with_tokens(
                    Some("same-token-123456".to_owned()),
                    Some("same-token-123456".to_owned()),
                    Some("same-token-123456".to_owned()),
                )
                .is_production_ready()
        );
    }

    #[test]
    fn expired_role_tokens_fail_closed() {
        let config = AuthConfig::disabled()
            .required(true)
            .with_tokens(
                Some("viewer-token-123456".to_owned()),
                Some("operator-token-123456".to_owned()),
                Some("admin-token-123456".to_owned()),
            )
            .with_token_ttl(Duration::ZERO);
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer admin-token-123456".parse().unwrap(),
        );
        assert_eq!(config.role(&headers), None);
    }

    #[test]
    fn metrics_render_exposes_all_counters() {
        let metrics = ApiMetrics::default();
        metrics.requests_total.fetch_add(2, Ordering::Relaxed);
        metrics.requests_failed.fetch_add(1, Ordering::Relaxed);
        metrics.events_published.fetch_add(3, Ordering::Relaxed);
        let rendered = metrics.render();
        assert!(rendered.contains("lxcup_http_requests_total 2"));
        assert!(rendered.contains("lxcup_http_requests_failed_total 1"));
        assert!(rendered.contains("lxcup_events_published_total 3"));
        assert!(rendered.contains("lxcup_uptime_seconds"));
    }

    #[test]
    fn default_authenticated_is_true() {
        assert!(default_authenticated());
    }
}

pub(super) async fn run_scan(
    State(state): State<ApiState>,
    Path(scan_id): Path<String>,
) -> Result<Json<ApiEnvelope<ScanDto>>, ApiError> {
    let scan_id = parse_uuid(&scan_id, "scan id")?;
    let scan_id = ScanId::from_uuid(scan_id);
    let (container_id, agent) = {
        let mut store = state.store.write().await;
        let scan = store
            .scans
            .iter_mut()
            .find(|scan| scan.id == scan_id)
            .ok_or_else(|| ApiError::not_found("scan not found"))?;
        scan.transition_to(lxcup_core::ScanStatus::Running, chrono::Utc::now())
            .map_err(|_| ApiError::bad_request("scan_not_runnable", "scan is not runnable"))?;
        let container_id = scan.container_id;
        let agent = state
            .agents
            .read()
            .await
            .get(&container_id)
            .cloned()
            .ok_or_else(|| {
                ApiError::dependency(
                    "agent_unavailable",
                    "no agent is registered for this container",
                )
            })?;
        (container_id, agent)
    };
    let response = agent
        .client
        .command(&AgentCommandRequest {
            action: AgentAction::Scan,
            packages: Vec::new(),
            idempotency_key: scan_id.as_uuid().to_string(),
        })
        .await
        .map_err(|_| {
            ApiError::dependency("agent_request_failed", "the agent scan request failed")
        })?;
    let mut store = state.store.write().await;
    let scan = store
        .scans
        .iter_mut()
        .find(|scan| scan.id == scan_id)
        .ok_or_else(|| ApiError::not_found("scan not found"))?;
    if response.success {
        match lxcup_apt::build_scan(
            container_id,
            lxcup_apt::AptExecutionResult {
                stdout: response.stdout,
                stderr: response.stderr,
                exit_code: Some(response.exit_code),
            },
        ) {
            Ok(mut result) => {
                result.scan.id = scan_id;
                *scan = result.scan;
            }
            Err(_) => {
                scan.transition_to(lxcup_core::ScanStatus::Failed, chrono::Utc::now())
                    .map_err(|_| {
                        ApiError::bad_request("scan_failed", "the agent returned invalid scan data")
                    })?;
            }
        }
    } else {
        scan.transition_to(lxcup_core::ScanStatus::Failed, chrono::Utc::now())
            .map_err(|_| ApiError::bad_request("scan_failed", "the agent scan failed"))?;
    }
    let dto = ScanDto::from(&*scan);
    drop(store);
    state.publish(ApiEvent::status(
        "scan",
        scan_id.as_uuid().to_string(),
        dto.status.clone(),
    ));
    Ok(Json(envelope(dto)))
}
