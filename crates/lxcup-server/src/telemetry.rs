use super::{ApiEnvelope, ApiError, ApiState, envelope, parse_uuid};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use lxcup_core::TargetId;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct TelemetryDto {
    pub target_id: TargetId,
    pub samples: Vec<lxcup_agent::SystemTelemetrySample>,
    pub collected_at: Option<DateTime<Utc>>,
}

pub(super) async fn get_target_telemetry(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<TelemetryDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    if !state
        .store
        .read()
        .await
        .targets
        .iter()
        .any(|target| target.id == target_id)
    {
        return Err(ApiError::not_found("target not found"));
    }
    let samples = match state.repositories.as_ref() {
        Some(repositories) => repositories
            .telemetry
            .list_recent(target_id)
            .await
            .map_err(|_| ApiError::storage())?,
        None => Vec::new(),
    };
    Ok(Json(envelope(TelemetryDto {
        target_id,
        collected_at: samples.last().map(|sample| sample.collected_at),
        samples,
    })))
}
