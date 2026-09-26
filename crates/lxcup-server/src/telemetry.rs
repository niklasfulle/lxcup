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
    pub partial: bool,
    pub missing_samples: usize,
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
    let cutoff =
        Utc::now() - chrono::Duration::seconds(lxcup_core::telemetry::HISTORY_WINDOW_SECONDS);
    in_memory_samples.retain(|sample| sample.collected_at >= cutoff);
    in_memory_samples.sort_by_key(|sample| sample.collected_at);
    let report_partial = {
        let store = state.store.read().await;
        store
            .agent_reports
            .get(&target_id)
            .is_some_and(|heartbeat| heartbeat.telemetry.partial)
    };
    let samples = match state.repositories.as_ref() {
        Some(repositories) => repositories
            .telemetry
            .list_recent(target_id)
            .await
            .map_err(|_| ApiError::storage())?,
        None => in_memory_samples,
    };
    let missing_samples = samples
        .windows(2)
        .map(|pair| (pair[1].collected_at - pair[0].collected_at).num_milliseconds())
        .filter(|gap_ms| *gap_ms > 7_500)
        .map(|gap_ms| ((gap_ms + 4_999) / 5_000 - 1) as usize)
        .sum::<usize>();
    Ok(Json(envelope(TelemetryDto {
        target_id,
        collected_at: samples.last().map(|sample| sample.collected_at),
        partial: report_partial || missing_samples > 0,
        missing_samples,
        samples,
    })))
}
