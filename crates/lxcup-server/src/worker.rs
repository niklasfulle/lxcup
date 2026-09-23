use super::{ApiEnvelope, ApiError, ApiState, State, envelope};
use axum::Json;
use serde::Serialize;

const WORKER_HEARTBEAT_WINDOW_SECONDS: i64 = 10;

#[derive(Clone, Debug, Serialize)]
pub struct WorkerAvailabilityDto {
    pub available: bool,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Reports whether any worker has polled the shared job queue recently.
pub(super) async fn get_worker_availability(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<WorkerAvailabilityDto>>, ApiError> {
    let Some(repositories) = state.repositories else {
        return Ok(Json(envelope(WorkerAvailabilityDto {
            available: false,
            last_seen_at: None,
        })));
    };
    let last_seen_at = repositories
        .worker_heartbeats
        .latest_since(
            chrono::Utc::now() - chrono::Duration::seconds(WORKER_HEARTBEAT_WINDOW_SECONDS),
        )
        .await
        .map_err(|_| ApiError::storage())?;
    Ok(Json(envelope(WorkerAvailabilityDto {
        available: last_seen_at.is_some(),
        last_seen_at,
    })))
}
