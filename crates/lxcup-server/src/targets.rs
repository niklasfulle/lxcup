use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, Permission, SecretId, Target, TargetId, TargetKind,
    TargetState, TargetTransport, envelope, map_secret_error, parse_uuid,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::{HeaderMap, StatusCode},
};
use lxcup_agent::AgentHeartbeat;
use lxcup_core::ActorRole;
use serde::{Deserialize, Serialize};

#[path = "targets/heartbeat.rs"]
mod heartbeat;
use heartbeat::{
    persist_agent_heartbeat, sanitize_docker_telemetry_window, sanitize_reported_package_inventory,
    sanitize_telemetry_window, warn_on_incomplete_telemetry,
};

#[derive(Clone, Debug, Deserialize)]
pub struct CreateTargetRequest {
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default)]
    pub credential_secret_ref: Option<SecretId>,
    #[serde(default)]
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeleteTargetRequest {
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetDto {
    pub id: TargetId,
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    pub ssh_user: Option<String>,
    pub credential_secret_ref: Option<SecretId>,
    pub ssh_known_hosts_secret_ref: Option<SecretId>,
    pub agent_secret_ref: SecretId,
    pub state: TargetState,
    /// Version reported by the most recent authenticated agent heartbeat.
    /// This remains absent until the agent has connected at least once.
    pub agent_version: Option<String>,
    /// Timestamp from the most recent authenticated heartbeat, not target config changes.
    pub agent_last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Version of the agent artifact shipped with this controller build.
    pub latest_agent_version: String,
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
            agent_last_seen_at: None,
            latest_agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at: target.created_at,
            updated_at: target.updated_at,
        }
    }
}

impl TargetDto {
    fn with_agent_report(target: &Target, report: Option<&AgentHeartbeat>) -> Self {
        let mut dto = Self::from(target);
        dto.agent_version = report.map(|heartbeat| heartbeat.info.version.clone());
        dto.agent_last_seen_at = report.map(|heartbeat| heartbeat.sent_at);
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
    let mut target = match (request.transport, request.credential_secret_ref) {
        (TargetTransport::Agent, None) if request.kind == TargetKind::WindowsServer => {
            Target::new_agent_only(
                request.name,
                request.kind,
                request.address,
                request.agent_secret_ref,
            )
        }
        (TargetTransport::Agent, _) => {
            return Err(ApiError::bad_request(
                "invalid_target",
                "agent transport is only supported for Windows targets without deployment credentials",
            ));
        }
        (_, Some(secret_ref)) => Target::new(
            request.name,
            request.kind,
            request.address,
            request.transport,
            secret_ref,
            request.agent_secret_ref,
        ),
        (_, None) => {
            return Err(ApiError::bad_request(
                "invalid_target",
                "deployment credential secret is required for this transport",
            ));
        }
    }
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

pub(super) async fn delete_target(
    State(state): State<ApiState>,
    Extension(actor_role): Extension<ActorRole>,
    Path(target_id): Path<String>,
    JsonBody(request): JsonBody<DeleteTargetRequest>,
) -> Result<StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "removing a target requires explicit confirmation",
        ));
    }
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let _scheduler_guard = state.scheduler_lock.lock().await;
    let mut coordinator = state.ansible.write().await;
    if coordinator.target_has_active_jobs(target_id) {
        return Err(ApiError::conflict(
            "target_busy",
            "wait for active workflows to finish before removing this target",
        ));
    }
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("target not found"))?;

    if let Some(repositories) = state.repositories.as_ref() {
        match repositories
            .targets
            .delete_with_related_data(target_id, &format!("{actor_role:?}").to_lowercase())
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(ApiError::not_found("target not found")),
            Err(lxcup_persistence::RepositoryError::TargetBusy) => {
                return Err(ApiError::conflict(
                    "target_busy",
                    "wait for active workflows to finish before removing this target",
                ));
            }
            Err(_) => return Err(ApiError::storage()),
        }
    }

    coordinator
        .remove_target_jobs(target_id)
        .map_err(|_| ApiError::conflict("target_busy", "the target has active workflows"))?;
    {
        let mut store = state.store.write().await;
        store.targets.retain(|item| item.id != target_id);
        store.agent_reports.remove(&target_id);
        store.target_docker_inventories.remove(&target_id);
        store.schedules.iter_mut().for_each(|schedule| {
            schedule.target_ids.retain(|id| *id != target_id);
        });
        store
            .schedules
            .retain(|schedule| !schedule.target_ids.is_empty());
        let removed_policy_ids = store
            .update_policies
            .iter()
            .filter(|policy| {
                policy.allowed_targets.len() == 1 && policy.allowed_targets[0] == target_id
            })
            .map(|policy| policy.id.clone())
            .collect::<std::collections::HashSet<_>>();
        store.update_policies.iter_mut().for_each(|policy| {
            policy.allowed_targets.retain(|id| *id != target_id);
        });
        store
            .update_policies
            .retain(|policy| !removed_policy_ids.contains(&policy.id));
        store.schedules.retain(|schedule| {
            !schedule
                .policy_id
                .as_ref()
                .is_some_and(|id| removed_policy_ids.contains(id))
        });
        store
            .enrollments
            .retain(|enrollment| enrollment.target_id != Some(target_id));
        let retained_enrollments = store
            .enrollments
            .iter()
            .map(|item| item.id)
            .collect::<std::collections::HashSet<_>>();
        store
            .enrollment_keys
            .retain(|_, id| retained_enrollments.contains(id));
    }
    drop(coordinator);
    state.publish(ApiEvent::status(
        "target",
        target.id.as_uuid().to_string(),
        "deleted",
    ));
    Ok(StatusCode::NO_CONTENT)
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
    let package_inventory = sanitize_reported_package_inventory(&heartbeat, target.kind, target.id);
    let telemetry_summary = sanitize_telemetry_window(&mut heartbeat);
    sanitize_docker_telemetry_window(&mut heartbeat);
    heartbeat.package_inventory = None;
    warn_on_incomplete_telemetry(target.id, &telemetry_summary);
    target.mark_managed();
    let persisted = target.clone();
    store.agent_reports.insert(persisted.id, heartbeat);
    let heartbeat_for_persistence = store.agent_reports.get(&persisted.id).cloned();
    if let Some(snapshot) = package_inventory.as_ref() {
        store
            .package_inventories
            .insert(persisted.id, snapshot.clone());
    }
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        persist_agent_heartbeat(
            &repositories,
            &persisted,
            heartbeat_for_persistence.as_ref(),
            package_inventory.as_ref(),
            telemetry_summary,
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod telemetry_sanitization_tests {
    use super::{heartbeat::TelemetrySanitizationSummary, sanitize_telemetry_window};
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
            docker_telemetry: Default::default(),
            package_inventory: None,
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
            docker_telemetry: Default::default(),
            package_inventory: None,
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
            docker_telemetry: Default::default(),
            package_inventory: None,
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
#[path = "targets_tests.rs"]
mod tests;
