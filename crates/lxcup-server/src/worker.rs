use super::{ApiEnvelope, ApiError, ApiState, State, envelope};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use lxcup_persistence::WorkerHeartbeatStatus;
use serde::Serialize;

const WORKER_HEARTBEAT_WINDOW_SECONDS: i64 = 10;

#[derive(Clone, Debug, Serialize)]
pub struct WorkerAvailabilityDto {
    pub available: bool,
    pub worker_version: Option<String>,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub artifact_store_available: Option<bool>,
    pub artifact_store_checked_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Reports whether any worker has polled the shared job queue recently.
pub(super) async fn get_worker_availability(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<WorkerAvailabilityDto>>, ApiError> {
    let Some(repositories) = state.repositories else {
        return Ok(Json(envelope(availability_dto(None, Utc::now()))));
    };
    let latest = repositories
        .worker_heartbeats
        .latest_status()
        .await
        .map_err(|_| ApiError::storage())?;
    Ok(Json(envelope(availability_dto(latest, Utc::now()))))
}

fn availability_dto(
    latest: Option<WorkerHeartbeatStatus>,
    now: DateTime<Utc>,
) -> WorkerAvailabilityDto {
    let available = latest.as_ref().is_some_and(|status| {
        status.last_seen_at >= now - Duration::seconds(WORKER_HEARTBEAT_WINDOW_SECONDS)
    });
    WorkerAvailabilityDto {
        available,
        worker_version: latest
            .as_ref()
            .and_then(|status| status.worker_version.clone()),
        last_seen_at: latest.as_ref().map(|status| status.last_seen_at),
        artifact_store_available: latest
            .as_ref()
            .and_then(|status| status.artifact_store_available),
        artifact_store_checked_at: latest.and_then(|status| status.artifact_store_checked_at),
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
