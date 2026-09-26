use super::{ApiEnvelope, ApiError, ApiState, Target, envelope};
use axum::{Json, extract::State};
use chrono::{DateTime, Duration, Utc};
use lxcup_agent::{AgentHeartbeat, SystemTelemetrySample};
use lxcup_core::{
    TargetId, TargetState,
    telemetry_alerts::{
        TelemetryAlertSeverity, TelemetryLoadSample, TelemetryThresholdAlert,
        evaluate_telemetry_thresholds,
    },
};
use serde::Serialize;

const TELEMETRY_STALE_AFTER: Duration = Duration::minutes(2);

#[derive(Clone, Debug, Serialize)]
pub struct TelemetryAlertDto {
    pub id: String,
    pub target_id: TargetId,
    pub target_name: String,
    pub target_kind: lxcup_core::TargetKind,
    pub target_address: String,
    pub metric: String,
    pub severity: TelemetryAlertSeverity,
    pub value_basis_points: Option<u16>,
    pub threshold_basis_points: Option<u16>,
    pub age_seconds: Option<i64>,
    pub triggered_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
}

pub(super) async fn list_telemetry_alerts(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<TelemetryAlertDto>>>, ApiError> {
    let (targets, reports) = {
        let store = state.store.read().await;
        (store.targets.clone(), store.agent_reports.clone())
    };
    let now = Utc::now();
    let mut alerts = Vec::new();
    for target in targets
        .iter()
        .filter(|target| target.state == TargetState::Managed)
    {
        let report = reports.get(&target.id);
        let samples = target_samples(&state, target, report).await?;
        let load_samples = samples.iter().map(load_sample).collect::<Vec<_>>();
        alerts.extend(
            evaluate_telemetry_thresholds(&load_samples)
                .into_iter()
                .map(|alert| threshold_alert(target, alert)),
        );
        if let Some(alert) = stale_alert(target, &samples, report, now) {
            alerts.push(alert);
        }
    }
    alerts.sort_by(|left, right| right.triggered_at.cmp(&left.triggered_at));
    Ok(Json(envelope(alerts)))
}

async fn target_samples(
    state: &ApiState,
    target: &Target,
    report: Option<&AgentHeartbeat>,
) -> Result<Vec<SystemTelemetrySample>, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        return repositories
            .telemetry
            .list_recent(target.id)
            .await
            .map_err(|_| ApiError::storage());
    }
    Ok(report
        .map(|heartbeat| heartbeat.telemetry.samples.clone())
        .unwrap_or_default())
}

fn load_sample(sample: &SystemTelemetrySample) -> TelemetryLoadSample {
    TelemetryLoadSample {
        collected_at: sample.collected_at,
        cpu_basis_points: sample.cpu_basis_points,
        memory_basis_points: sample.memory_basis_points,
        storage_basis_points: sample.storage_basis_points,
    }
}

fn threshold_alert(target: &Target, alert: TelemetryThresholdAlert) -> TelemetryAlertDto {
    TelemetryAlertDto {
        id: format!("{}:{:?}", target.id.as_uuid(), alert.metric).to_ascii_lowercase(),
        target_id: target.id,
        target_name: target.name.clone(),
        target_kind: target.kind,
        target_address: target.address.clone(),
        metric: alert.metric.label().to_owned(),
        severity: alert.severity,
        value_basis_points: Some(alert.value_basis_points),
        threshold_basis_points: Some(alert.threshold_basis_points),
        age_seconds: None,
        triggered_at: alert.triggered_at,
        observed_at: alert.observed_at,
    }
}

fn stale_alert(
    target: &Target,
    samples: &[SystemTelemetrySample],
    report: Option<&AgentHeartbeat>,
    now: DateTime<Utc>,
) -> Option<TelemetryAlertDto> {
    let last_signal = samples
        .iter()
        .map(|sample| sample.collected_at)
        .max()
        .or_else(|| report.map(|heartbeat| heartbeat.sent_at))?;
    let age = now.signed_duration_since(last_signal).num_seconds().max(0);
    (age > TELEMETRY_STALE_AFTER.num_seconds()).then(|| TelemetryAlertDto {
        id: format!("{}:telemetry_stale", target.id.as_uuid()),
        target_id: target.id,
        target_name: target.name.clone(),
        target_kind: target.kind,
        target_address: target.address.clone(),
        metric: "Telemetrie".to_owned(),
        severity: TelemetryAlertSeverity::Warning,
        value_basis_points: None,
        threshold_basis_points: None,
        age_seconds: Some(age),
        triggered_at: last_signal + TELEMETRY_STALE_AFTER,
        observed_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{SecretId, TargetKind, TargetTransport, telemetry_alerts::TelemetryMetric};

    fn target() -> Target {
        Target::new(
            "test-lxc",
            TargetKind::Lxc,
            "192.0.2.4",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap()
    }

    fn sample(at: DateTime<Utc>) -> SystemTelemetrySample {
        SystemTelemetrySample {
            collected_at: at,
            cpu_basis_points: Some(9_500),
            memory_basis_points: Some(2_000),
            storage_basis_points: Some(2_000),
            load_1_milli: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            process_count: None,
        }
    }

    #[test]
    fn threshold_alert_has_stable_resource_metric_identity_and_context() {
        let target = target();
        let alert = TelemetryThresholdAlert {
            metric: TelemetryMetric::Cpu,
            severity: TelemetryAlertSeverity::Warning,
            value_basis_points: 9_200,
            threshold_basis_points: 9_000,
            triggered_at: Utc::now(),
            observed_at: Utc::now(),
        };
        let dto = threshold_alert(&target, alert);
        assert_eq!(dto.id, format!("{}:cpu", target.id.as_uuid()));
        assert_eq!(dto.target_name, "test-lxc");
        assert_eq!(dto.metric, "CPU");
        assert_eq!(dto.value_basis_points, Some(9_200));
    }

    #[test]
    fn stale_telemetry_warns_after_two_minutes_and_is_quiet_before_then() {
        let target = target();
        let now = Utc::now();
        assert!(stale_alert(&target, &[sample(now - Duration::seconds(119))], None, now).is_none());
        let alert =
            stale_alert(&target, &[sample(now - Duration::seconds(150))], None, now).unwrap();
        assert_eq!(alert.metric, "Telemetrie");
        assert_eq!(alert.age_seconds, Some(150));
        assert_eq!(alert.triggered_at, now - Duration::seconds(30));
    }
}
