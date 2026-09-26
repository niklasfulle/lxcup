use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, Permission, SecretId, Target, TargetId, TargetKind,
    TargetState, TargetTransport, envelope, map_secret_error, parse_uuid,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::Timelike;
use lxcup_agent::AgentHeartbeat;
use lxcup_core::ActorRole;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct CreateTargetRequest {
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    #[serde(default)]
    pub ssh_user: Option<String>,
    pub credential_secret_ref: SecretId,
    #[serde(default)]
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDto {
    pub id: TargetId,
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    pub ssh_user: Option<String>,
    pub credential_secret_ref: SecretId,
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
    pub state: TargetState,
    /// Version reported by the most recent authenticated agent heartbeat.
    /// This remains absent until the agent has connected at least once.
    pub agent_version: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&Target> for TargetDto {
    fn from(target: &Target) -> Self {
        Self {
            id: target.id,
            name: target.name.clone(),
            kind: target.kind,
            address: target.address.clone(),
            transport: target.transport,
            ssh_user: target.ssh_user.clone(),
            credential_secret_ref: target.credential_secret_ref,
            ssh_known_hosts_secret_ref: target.ssh_known_hosts_secret_ref,
            agent_secret_ref: target.agent_secret_ref,
            state: target.state,
            agent_version: None,
            created_at: target.created_at,
            updated_at: target.updated_at,
        }
    }
}

impl TargetDto {
    fn with_agent_report(target: &Target, report: Option<&AgentHeartbeat>) -> Self {
        let mut dto = Self::from(target);
        dto.agent_version = report.map(|heartbeat| heartbeat.info.version.clone());
        dto
    }
}

pub(super) async fn list_targets(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<Vec<TargetDto>>> {
    let store = state.store.read().await;
    Json(envelope(
        store
            .targets
            .iter()
            .map(|target| TargetDto::with_agent_report(target, store.agent_reports.get(&target.id)))
            .collect(),
    ))
}

pub(super) async fn create_target(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateTargetRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<TargetDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let mut target = Target::new(
        request.name,
        request.kind,
        request.address,
        request.transport,
        request.credential_secret_ref,
        request.agent_secret_ref,
    )
    .map_err(|_| {
        ApiError::bad_request("invalid_target", "target fields or transport are invalid")
    })?;
    target.ssh_user = request.ssh_user.filter(|value| !value.trim().is_empty());
    target.ssh_known_hosts_secret_ref = request.ssh_known_hosts_secret_ref;
    let dto = TargetDto::from(&target);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .save(&target)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.store.write().await.targets.push(target);
    state.ensure_default_update_policies().await?;
    state.publish(ApiEvent::status(
        "target",
        dto.id.as_uuid().to_string(),
        "pending",
    ));
    Ok((StatusCode::CREATED, Json(envelope(dto))))
}

pub(super) async fn get_target(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<TargetDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let store = state.store.read().await;
    let target = store
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    Ok(Json(envelope(TargetDto::with_agent_report(
        target,
        store.agent_reports.get(&target.id),
    ))))
}

pub(super) async fn receive_agent_heartbeat(
    State(state): State<ApiState>,
    headers: HeaderMap,
    JsonBody(mut heartbeat): JsonBody<AgentHeartbeat>,
) -> Result<StatusCode, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let mut store = state.store.write().await;
    let target = store
        .targets
        .iter_mut()
        .find(|target| target.id.as_uuid() == heartbeat.target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    let expected = state
        .secrets
        .read(target.agent_secret_ref)
        .map_err(map_secret_error)?;
    if expected.expose() != token {
        return Err(ApiError::unauthorized());
    }
    let telemetry_summary = sanitize_telemetry_window(&mut heartbeat);
    if telemetry_summary.rejected() > 0 || telemetry_summary.missing_samples > 0 {
        tracing::warn!(
            target: "lxcup_server::telemetry",
            target_id = %target.id.as_uuid(),
            received_samples = telemetry_summary.received,
            rejected_samples = telemetry_summary.rejected(),
            stale_samples = telemetry_summary.stale,
            future_samples = telemetry_summary.future,
            out_of_range_samples = telemetry_summary.out_of_range,
            missing_samples = telemetry_summary.missing_samples,
            duplicate_samples = telemetry_summary.duplicates,
            truncated_samples = telemetry_summary.truncated,
            "agent telemetry window was incomplete or contained rejected samples"
        );
    }
    target.mark_managed();
    let persisted = target.clone();
    store.agent_reports.insert(persisted.id, heartbeat);
    let heartbeat_for_persistence = store.agent_reports.get(&persisted.id).cloned();
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .targets
            .update(&persisted)
            .await
            .map_err(|_| ApiError::storage())?;
        if let Some(heartbeat) = heartbeat_for_persistence.as_ref() {
            if repositories
                .telemetry
                .append_heartbeat(heartbeat)
                .await
                .is_err()
            {
                tracing::error!(
                    target: "lxcup_server::telemetry",
                    target_id = %persisted.id.as_uuid(),
                    accepted_samples = telemetry_summary.accepted,
                    "agent telemetry persistence failed"
                );
                return Err(ApiError::storage());
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Treat telemetry as best-effort heartbeat data: discard malformed, stale,
/// duplicate, or oversized sample windows while preserving the heartbeat.
/// This bounds persistence work even for an authenticated but buggy agent.
#[derive(Debug, Default, Eq, PartialEq)]
struct TelemetrySanitizationSummary {
    received: usize,
    accepted: usize,
    stale: usize,
    future: usize,
    out_of_range: usize,
    missing_samples: usize,
    duplicates: usize,
    truncated: usize,
}

impl TelemetrySanitizationSummary {
    fn rejected(&self) -> usize {
        self.stale + self.future + self.out_of_range + self.duplicates + self.truncated
    }
}

fn sanitize_telemetry_window(heartbeat: &mut AgentHeartbeat) -> TelemetrySanitizationSummary {
    const MAX_SAMPLES: usize = 32;
    let samples = &mut heartbeat.telemetry.samples;
    let mut summary = TelemetrySanitizationSummary {
        received: samples.len(),
        ..TelemetrySanitizationSummary::default()
    };
    if samples.len() > MAX_SAMPLES {
        summary.truncated = samples.len() - MAX_SAMPLES;
        samples.drain(..samples.len() - MAX_SAMPLES);
        heartbeat.telemetry.partial = true;
    }

    let received_at = chrono::Utc::now();
    let max_sample_age = chrono::Duration::seconds(lxcup_agent::TelemetryBuffer::WINDOW_SECONDS);
    let max_future_skew = chrono::Duration::seconds(-5);
    let sent_at = heartbeat.sent_at;
    samples.retain_mut(|sample| {
        let sample_age = sent_at.signed_duration_since(sample.collected_at);
        if sample_age > max_sample_age {
            summary.stale += 1;
            return false;
        }
        if sample_age < max_future_skew {
            summary.future += 1;
            return false;
        }
        if sample.cpu_basis_points.is_some_and(|value| value > 10_000)
            || sample
                .memory_basis_points
                .is_some_and(|value| value > 10_000)
            || sample
                .storage_basis_points
                .is_some_and(|value| value > 10_000)
        {
            summary.out_of_range += 1;
            return false;
        }
        let normalized_at = received_at - sample_age;
        // Strip transport-jitter fractions so overlapping heartbeats hit the
        // same target/timestamp database key and are deduplicated on insert.
        sample.collected_at = normalized_at.with_nanosecond(0).unwrap_or(normalized_at);
        true
    });
    if summary.stale + summary.future + summary.out_of_range > 0 {
        heartbeat.telemetry.partial = true;
    }

    samples.sort_by_key(|sample| sample.collected_at);
    samples.dedup_by(|right, left| {
        let duplicate = right.collected_at == left.collected_at;
        if duplicate {
            summary.duplicates += 1;
        }
        duplicate
    });
    if summary.duplicates > 0 {
        heartbeat.telemetry.partial = true;
    }
    summary.missing_samples = samples
        .windows(2)
        .map(|pair| (pair[1].collected_at - pair[0].collected_at).num_milliseconds())
        .filter(|gap_ms| *gap_ms > 7_500)
        .map(|gap_ms| ((gap_ms + 4_999) / 5_000 - 1) as usize)
        .sum();
    if summary.missing_samples > 0 {
        heartbeat.telemetry.partial = true;
    }
    summary.accepted = samples.len();
    summary
}

#[cfg(test)]
mod telemetry_sanitization_tests {
    use super::{TelemetrySanitizationSummary, sanitize_telemetry_window};
    use chrono::Timelike;
    use chrono::{Duration, Utc};
    use lxcup_agent::{
        AgentHeartbeat, AgentInfo, AgentMetrics, AgentPlatform, SystemTelemetrySample,
        SystemTelemetryWindow,
    };

    fn sample(
        collected_at: chrono::DateTime<Utc>,
        cpu_basis_points: Option<u16>,
    ) -> SystemTelemetrySample {
        SystemTelemetrySample {
            collected_at,
            cpu_basis_points,
            memory_basis_points: Some(5_000),
            storage_basis_points: Some(7_000),
            load_1_milli: Some(1_000),
            network_rx_bytes: Some(100),
            network_tx_bytes: Some(200),
            process_count: Some(50),
        }
    }

    #[test]
    fn telemetry_summary_identifies_samples_rejected_by_each_guard() {
        let now = Utc::now();
        let mut heartbeat = AgentHeartbeat {
            target_id: uuid::Uuid::new_v4(),
            info: AgentInfo {
                agent_id: "test-agent".to_owned(),
                platform: AgentPlatform::Linux,
                hostname: "test-host".to_owned(),
                version: "0.3.1".to_owned(),
                protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
            },
            metrics: AgentMetrics {
                collected_at: now,
                commands_total: 0,
                commands_failed: 0,
                last_command_at: None,
            },
            sent_at: now,
            telemetry: SystemTelemetryWindow {
                samples: vec![
                    sample(now, Some(4_000)),
                    sample(now, Some(3_000)),
                    sample(now - Duration::seconds(61), Some(2_000)),
                    sample(now + Duration::seconds(6), Some(2_000)),
                    sample(now, Some(10_001)),
                ],
                partial: false,
            },
        };

        let summary = sanitize_telemetry_window(&mut heartbeat);

        assert_eq!(
            summary,
            TelemetrySanitizationSummary {
                received: 5,
                accepted: 1,
                stale: 1,
                future: 1,
                out_of_range: 1,
                missing_samples: 0,
                duplicates: 1,
                truncated: 0,
            }
        );
        assert!(heartbeat.telemetry.partial);
        assert_eq!(heartbeat.telemetry.samples.len(), 1);
    }

    #[test]
    fn overlapping_telemetry_window_accepts_sixty_seconds_and_flags_missing_samples() {
        let now = Utc::now();
        let mut heartbeat = AgentHeartbeat {
            target_id: uuid::Uuid::new_v4(),
            info: AgentInfo {
                agent_id: "test-agent".to_owned(),
                platform: AgentPlatform::Linux,
                hostname: "test-host".to_owned(),
                version: "0.3.1".to_owned(),
                protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
            },
            metrics: AgentMetrics {
                collected_at: now,
                commands_total: 0,
                commands_failed: 0,
                last_command_at: None,
            },
            sent_at: now,
            telemetry: SystemTelemetryWindow {
                samples: vec![
                    sample(now - Duration::seconds(60), Some(2_000)),
                    sample(now - Duration::seconds(55), Some(2_000)),
                    sample(now - Duration::seconds(40), Some(2_000)),
                    sample(now, Some(2_000)),
                ],
                partial: false,
            },
        };

        let summary = sanitize_telemetry_window(&mut heartbeat);

        assert_eq!(summary.accepted, 4);
        assert_eq!(summary.stale, 0);
        assert_eq!(summary.missing_samples, 9);
        assert!(heartbeat.telemetry.partial);
        assert!(
            heartbeat
                .telemetry
                .samples
                .iter()
                .all(|sample| sample.collected_at.nanosecond() == 0)
        );
    }

    #[test]
    fn telemetry_sample_times_are_normalized_when_agent_clock_is_behind() {
        let server_now = Utc::now();
        let agent_now = server_now - Duration::minutes(2);
        let mut heartbeat = AgentHeartbeat {
            target_id: uuid::Uuid::new_v4(),
            info: AgentInfo {
                agent_id: "test-agent".to_owned(),
                platform: AgentPlatform::Linux,
                hostname: "test-host".to_owned(),
                version: "0.3.1".to_owned(),
                protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
            },
            metrics: AgentMetrics {
                collected_at: agent_now,
                commands_total: 0,
                commands_failed: 0,
                last_command_at: None,
            },
            sent_at: agent_now,
            telemetry: SystemTelemetryWindow {
                samples: vec![
                    sample(agent_now - Duration::seconds(5), Some(4_000)),
                    sample(agent_now - Duration::seconds(10), Some(3_000)),
                ],
                partial: false,
            },
        };

        let summary = sanitize_telemetry_window(&mut heartbeat);

        assert_eq!(summary.accepted, 2);
        assert_eq!(summary.rejected(), 0);
        let ages = heartbeat
            .telemetry
            .samples
            .iter()
            .map(|sample| (Utc::now() - sample.collected_at).num_seconds())
            .collect::<Vec<_>>();
        assert_eq!(ages, [10, 5]);
    }
}

pub(super) fn require_permission(role: ActorRole, permission: Permission) -> Result<(), ApiError> {
    if role.grants(permission) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "permission_denied",
            "the role cannot change this resource",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Extension;

    #[tokio::test]
    async fn creating_a_target_also_creates_its_standard_update_policy() {
        let state = ApiState::new();
        let request = CreateTargetRequest {
            name: "onboarding-target".to_owned(),
            kind: TargetKind::LinuxServer,
            address: "192.0.2.80".to_owned(),
            transport: TargetTransport::Ssh,
            ssh_user: Some("lxcup".to_owned()),
            credential_secret_ref: SecretId::new(),
            ssh_known_hosts_secret_ref: Some(SecretId::new()),
            agent_secret_ref: SecretId::new(),
        };

        let (_, response) = create_target(
            State(state.clone()),
            Extension(ActorRole::Admin),
            JsonBody(request),
        )
        .await
        .unwrap();

        let target_id = response.0.data.id;
        let policies = state.store.read().await.update_policies.clone();
        assert_eq!(
            policies,
            vec![super::super::default_update_policy(vec![target_id])]
        );
    }
}
