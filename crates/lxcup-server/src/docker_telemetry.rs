use super::{ApiEnvelope, ApiError, ApiState, envelope, parse_uuid};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use lxcup_core::TargetId;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct DockerTelemetryDto {
    pub target_id: TargetId,
    pub samples: Vec<lxcup_agent::DockerTelemetrySample>,
    pub collected_at: Option<DateTime<Utc>>,
}

pub(super) async fn get_target_docker_telemetry(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<DockerTelemetryDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let in_memory_samples = {
        let store = state.store.read().await;
        if !store.targets.iter().any(|target| target.id == target_id) {
            return Err(ApiError::not_found("target not found"));
        }
        store
            .agent_reports
            .get(&target_id)
            .map(|heartbeat| heartbeat.docker_telemetry.samples.clone())
            .unwrap_or_default()
    };
    let samples = match state.repositories.as_ref() {
        Some(repositories) => repositories
            .telemetry
            .list_docker_recent(target_id)
            .await
            .map_err(|_| ApiError::storage())?,
        None => in_memory_samples,
    };
    Ok(Json(envelope(DockerTelemetryDto {
        target_id,
        collected_at: samples.iter().map(|sample| sample.collected_at).max(),
        samples,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};

    #[tokio::test]
    async fn docker_telemetry_requires_a_registered_target() {
        let state = ApiState::new();
        let invalid = get_target_docker_telemetry(State(state.clone()), Path("invalid".to_owned()))
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "invalid_id");
        let missing =
            get_target_docker_telemetry(State(state), Path(uuid::Uuid::new_v4().to_string()))
                .await
                .unwrap_err();
        assert_eq!(missing.code, "not_found");
    }
}
