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
    let mut in_memory_samples = {
        let store = state.store.read().await;
        if !store.targets.iter().any(|target| target.id == target_id) {
            return Err(ApiError::not_found("target not found"));
        }
        store
            .agent_reports
            .get(&target_id)
            .map(|heartbeat| heartbeat.telemetry.samples.clone())
            .unwrap_or_default()
    };
    let cutoff = Utc::now() - chrono::Duration::seconds(30);
    in_memory_samples.retain(|sample| sample.collected_at >= cutoff);
    in_memory_samples.sort_by_key(|sample| sample.collected_at);
    let samples = match state.repositories.as_ref() {
        Some(repositories) => repositories
            .telemetry
            .list_recent(target_id)
            .await
            .map_err(|_| ApiError::storage())?,
        None => in_memory_samples,
    };
    Ok(Json(envelope(TelemetryDto {
        target_id,
        collected_at: samples.last().map(|sample| sample.collected_at),
        samples,
    })))
}
