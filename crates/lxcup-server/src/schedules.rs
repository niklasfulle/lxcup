use super::{ApiEnvelope, ApiError, ApiState, ApiStore, envelope, require_permission};
use axum::{
    Json,
    extract::{Json as JsonBody, State},
};
use chrono::Utc;
use lxcup_core::{
    ActorRole, JobSchedule, Permission, ScheduleFrequency, SecretId, TargetId, ThresholdRule,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateScheduleRequest {
    pub id: String,
    pub operation: String,
    pub timezone: String,
    pub target_ids: Vec<TargetId>,
    pub every_minutes: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub threshold: Option<ThresholdRule>,
    #[serde(default)]
    pub policy_id: Option<String>,
    #[serde(default)]
    pub backup_secret_ref: Option<SecretId>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SetScheduleEnabledRequest {
    enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScheduleDto {
    pub id: String,
    pub operation: String,
    pub timezone: String,
    pub target_ids: Vec<TargetId>,
    pub every_minutes: u32,
    pub enabled: bool,
    pub threshold: Option<ThresholdRule>,
    pub policy_id: Option<String>,
    pub last_run_at: Option<chrono::DateTime<Utc>>,
    pub next_run_at: chrono::DateTime<Utc>,
    pub last_error: Option<String>,
}

impl From<&JobSchedule> for ScheduleDto {
    fn from(schedule: &JobSchedule) -> Self {
        let ScheduleFrequency::EveryMinutes(every_minutes) = schedule.frequency;
        Self {
            id: schedule.id.clone(),
            operation: schedule.operation.clone(),
            timezone: schedule.timezone.clone(),
            target_ids: schedule.target_ids.clone(),
            every_minutes,
            enabled: schedule.enabled,
            threshold: schedule.threshold,
            policy_id: schedule.policy_id.clone(),
            last_run_at: schedule.last_run_at,
            next_run_at: schedule.next_run_at,
            last_error: schedule.last_error.clone(),
        }
    }
}

fn default_true() -> bool {
    true
}

pub(super) async fn list_schedules(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<ScheduleDto>>>, ApiError> {
    let schedules = if let Some(repositories) = state.repositories.clone() {
        repositories
            .schedules
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.schedules.clone()
    };
    Ok(Json(envelope(
        schedules.iter().map(ScheduleDto::from).collect(),
    )))
}

pub(super) async fn create_schedule(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateScheduleRequest>,
) -> Result<(axum::http::StatusCode, Json<ApiEnvelope<ScheduleDto>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if request.operation == "create_backup" && actor_role != ActorRole::Admin {
        return Err(ApiError::forbidden(
            "admin_required",
            "administrator access is required to schedule backups",
        ));
    }
    validate_schedule_fields(&request)?;
    if request.operation == "create_backup" {
        let secret_ref = request.backup_secret_ref.ok_or_else(|| {
            ApiError::bad_request(
                "backup_secret_required",
                "select a backup-passphrase secret",
            )
        })?;
        super::backups::read_backup_passphrase(&state, secret_ref)?;
    }
    let mut store = state.store.write().await;
    validate_schedule_resources(&request, &store)?;
    let schedule = JobSchedule {
        id: request.id,
        operation: request.operation,
        timezone: request.timezone,
        target_ids: request.target_ids,
        frequency: ScheduleFrequency::EveryMinutes(request.every_minutes),
        enabled: request.enabled,
        threshold: request.threshold,
        policy_id: request.policy_id,
        backup_secret_ref: request.backup_secret_ref,
        last_run_at: None,
        next_run_at: Utc::now() + ScheduleFrequency::EveryMinutes(request.every_minutes).interval(),
        last_error: None,
    };
    let dto = ScheduleDto::from(&schedule);
    store.schedules.push(schedule);
    let persisted = store
        .schedules
        .last()
        .cloned()
        .expect("schedule was just inserted");
    drop(store);
    if let Some(repositories) = state.repositories.clone() {
        repositories
            .schedules
            .save(&persisted)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok((axum::http::StatusCode::CREATED, Json(envelope(dto))))
}

pub(super) async fn set_schedule_enabled(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    axum::extract::Path(schedule_id): axum::extract::Path<String>,
    JsonBody(request): JsonBody<SetScheduleEnabledRequest>,
) -> Result<Json<ApiEnvelope<ScheduleDto>>, ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    let updated = {
        let mut store = state.store.write().await;
        let schedule = store
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == schedule_id)
            .ok_or_else(|| ApiError::not_found("schedule not found"))?;
        schedule.enabled = request.enabled;
        schedule.clone()
    };
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .schedules
            .update(&updated)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    state.publish(super::ApiEvent::status(
        "schedule",
        updated.id.clone(),
        if updated.enabled {
            "enabled"
        } else {
            "disabled"
        },
    ));
    Ok(Json(envelope(ScheduleDto::from(&updated))))
}

fn validate_schedule_fields(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    validate_schedule_identity(request)?;
    validate_schedule_operation(request)?;
    validate_backup_schedule(request)
}

fn validate_schedule_identity(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    if request.id.trim().is_empty() || request.id.len() > 128 {
        return Err(ApiError::bad_request(
            "invalid_schedule",
            "schedule id is required and bounded",
        ));
    }
    if request.timezone.trim().is_empty() || request.timezone.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_timezone",
            "timezone is required and bounded",
        ));
    }
    if request.every_minutes == 0 || request.every_minutes > 7 * 24 * 60 {
        return Err(ApiError::bad_request(
            "invalid_interval",
            "schedule interval must be between 1 minute and 7 days",
        ));
    }
    Ok(())
}

fn validate_schedule_operation(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    if !matches!(
        request.operation.as_str(),
        "health_check"
            | "collect_package_inventory"
            | "update_packages"
            | "docker_discovery"
            | "create_backup"
    ) {
        return Err(ApiError::bad_request(
            "invalid_operation",
            "operation is not registered for scheduling",
        ));
    }
    if request.operation == "update_packages" && request.policy_id.is_none() {
        return Err(ApiError::bad_request(
            "update_policy_required",
            "package updates require an explicit update policy",
        ));
    }
    if request.operation == "docker_discovery" && request.threshold.is_some() {
        return Err(ApiError::bad_request(
            "invalid_threshold",
            "Docker discovery does not support telemetry thresholds",
        ));
    }
    if request.operation != "create_backup" && request.backup_secret_ref.is_some() {
        return Err(ApiError::bad_request(
            "invalid_backup_secret",
            "backup secrets are only valid for backup schedules",
        ));
    }
    Ok(())
}

fn validate_backup_schedule(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    if request.operation == "create_backup" {
        if request.backup_secret_ref.is_none() {
            return Err(ApiError::bad_request(
                "backup_secret_required",
                "scheduled backups require a global backup-passphrase secret",
            ));
        }
        if !request.target_ids.is_empty()
            || request.threshold.is_some()
            || request.policy_id.is_some()
        {
            return Err(ApiError::bad_request(
                "invalid_backup_schedule",
                "backup schedules do not use targets, thresholds, or update policies",
            ));
        }
        if request.timezone != "UTC" {
            return Err(ApiError::bad_request(
                "invalid_backup_schedule_timezone",
                "backup schedules use UTC",
            ));
        }
        if request.every_minutes < 60 {
            return Err(ApiError::bad_request(
                "backup_interval_too_short",
                "scheduled backups must run at least 60 minutes apart",
            ));
        }
    }
    Ok(())
}

fn validate_schedule_resources(
    request: &CreateScheduleRequest,
    store: &ApiStore,
) -> Result<(), ApiError> {
    if store
        .schedules
        .iter()
        .any(|schedule| schedule.id == request.id)
    {
        return Err(ApiError::conflict(
            "schedule_exists",
            "schedule id already exists",
        ));
    }
    if request.target_ids.is_empty() && request.operation != "create_backup" {
        return Err(ApiError::bad_request(
            "target_required",
            "at least one target is required",
        ));
    }
    if request
        .target_ids
        .iter()
        .any(|id| !store.targets.iter().any(|target| target.id == *id))
    {
        return Err(ApiError::not_found("schedule target not found"));
    }
    if request.operation == "docker_discovery"
        && request.target_ids.iter().any(|target_id| {
            !store
                .targets
                .iter()
                .any(|target| target.id == *target_id && target.kind == lxcup_core::TargetKind::Lxc)
        })
    {
        return Err(ApiError::bad_request(
            "docker_discovery_requires_lxc",
            "Docker discovery schedules require LXC targets",
        ));
    }
    validate_schedule_policy_targets(request, store)
}

fn validate_schedule_policy_targets(
    request: &CreateScheduleRequest,
    store: &ApiStore,
) -> Result<(), ApiError> {
    let Some(policy_id) = request.policy_id.as_deref() else {
        return Ok(());
    };
    let policy = store
        .update_policies
        .iter()
        .find(|policy| policy.id == policy_id && policy.enabled)
        .ok_or_else(|| ApiError::not_found("update policy not found or disabled"))?;
    if request
        .target_ids
        .iter()
        .any(|target_id| !policy.allowed_targets.contains(target_id))
    {
        return Err(ApiError::bad_request(
            "update_policy_target_denied",
            "update policy does not allow every schedule target",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{
        SecretId, SecretValue, Target, TargetKind, TargetTransport, UpdatePolicy, UpdateRisk,
    };
    use lxcup_secrets::{CreateSecret, InMemorySecretStore, SecretStore};
    use std::sync::Arc;

    fn request(operation: &str) -> CreateScheduleRequest {
        CreateScheduleRequest {
            id: "daily-check".to_owned(),
            operation: operation.to_owned(),
            timezone: "Europe/Berlin".to_owned(),
            target_ids: vec![TargetId::new()],
            every_minutes: 60,
            enabled: true,
            threshold: None,
            policy_id: None,
            backup_secret_ref: None,
        }
    }

    fn target(kind: TargetKind) -> Target {
        Target::new(
            "schedule-target",
            kind,
            "192.0.2.250",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap()
    }

    #[test]
    fn schedule_fields_reject_unbounded_or_unregistered_values() {
        let mut value = request("health_check");
        value.id.clear();
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_schedule"
        );
        value.id = "x".repeat(129);
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_schedule"
        );
        value.id = "valid".to_owned();
        value.timezone.clear();
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_timezone"
        );
        value.timezone = "z".repeat(65);
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_timezone"
        );
        value.timezone = "UTC".to_owned();
        value.every_minutes = 0;
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_interval"
        );
        value.every_minutes = 10081;
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_interval"
        );
        value.every_minutes = 60;
        value.operation = "unregistered".to_owned();
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_operation"
        );
        value.operation = "update_packages".to_owned();
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "update_policy_required"
        );
        value.policy_id = Some("safe".to_owned());
        value.operation = "docker_discovery".to_owned();
        value.threshold = Some(ThresholdRule {
            metric: lxcup_core::ThresholdMetric::CpuBasisPoints,
            operator: lxcup_core::ThresholdOperator::GreaterThanOrEqual,
            value: 8000,
        });
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_threshold"
        );
        value.threshold = None;
        assert!(validate_schedule_fields(&value).is_ok());
    }

    #[test]
    fn schedule_resources_validate_duplicates_targets_docker_kind_and_policy_scope() {
        let managed = target(TargetKind::LinuxServer);
        let lxc = target(TargetKind::Lxc);
        let mut store = ApiStore {
            targets: vec![managed.clone(), lxc.clone()],
            ..ApiStore::default()
        };

        let mut value = request("health_check");
        value.target_ids.clear();
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "target_required"
        );
        value.target_ids = vec![TargetId::new()];
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "not_found"
        );
        value.target_ids = vec![managed.id];
        value.operation = "docker_discovery".to_owned();
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "docker_discovery_requires_lxc"
        );
        value.target_ids = vec![lxc.id];
        assert!(validate_schedule_resources(&value, &store).is_ok());

        let mut duplicate = request("health_check");
        duplicate.target_ids = vec![managed.id];
        store.schedules.push(JobSchedule {
            id: duplicate.id.clone(),
            operation: "health_check".to_owned(),
            timezone: "UTC".to_owned(),
            target_ids: vec![managed.id],
            frequency: ScheduleFrequency::EveryMinutes(60),
            enabled: true,
            threshold: None,
            policy_id: None,
            backup_secret_ref: None,
            last_run_at: None,
            next_run_at: Utc::now(),
            last_error: None,
        });
        assert_eq!(
            validate_schedule_resources(&duplicate, &store)
                .unwrap_err()
                .code,
            "schedule_exists"
        );
        store.schedules.clear();

        value.operation = "health_check".to_owned();
        value.target_ids = vec![managed.id];
        value.policy_id = Some("missing".to_owned());
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "not_found"
        );
        store.update_policies.push(UpdatePolicy {
            id: "missing".to_owned(),
            allowed_targets: vec![lxc.id],
            allowed_packages: vec!["curl".to_owned()],
            maintenance_start_minute: 0,
            maintenance_end_minute: 1439,
            timezone: "UTC".to_owned(),
            maximum_risk: UpdateRisk::High,
            enabled: false,
        });
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "not_found"
        );
        store.update_policies[0].enabled = true;
        assert_eq!(
            validate_schedule_resources(&value, &store)
                .unwrap_err()
                .code,
            "update_policy_target_denied"
        );
        store.update_policies[0].allowed_targets.push(managed.id);
        assert!(validate_schedule_resources(&value, &store).is_ok());
    }

    #[tokio::test]
    async fn backup_schedule_requires_admin_and_never_returns_the_secret_reference() {
        let secrets = InMemorySecretStore::default();
        let created = secrets
            .create(CreateSecret {
                name: "backup-passphrase".to_owned(),
                kind: lxcup_core::SecretKind::BackupPassphrase,
                scope: lxcup_core::SecretScope::Global,
                value: SecretValue::new("correct horse battery staple".to_owned()).unwrap(),
            })
            .unwrap();
        let state = ApiState::new().with_secret_store(Arc::new(secrets));
        let mut backup = request("create_backup");
        backup.timezone = "UTC".to_owned();
        backup.target_ids.clear();
        backup.backup_secret_ref = Some(created.metadata.id);

        let denied = create_schedule(
            State(state.clone()),
            axum::Extension(ActorRole::Operator),
            JsonBody(backup.clone()),
        )
        .await
        .unwrap_err();
        assert_eq!(denied.status, axum::http::StatusCode::FORBIDDEN);

        let (status, Json(envelope)) = create_schedule(
            State(state),
            axum::Extension(ActorRole::Admin),
            JsonBody(backup),
        )
        .await
        .unwrap();
        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(envelope.data.operation, "create_backup");
        assert_eq!(envelope.data.target_ids, Vec::<TargetId>::new());
        assert!(
            !serde_json::to_string(&envelope.data)
                .unwrap()
                .contains("backup_secret_ref")
        );
    }

    #[test]
    fn backup_schedules_reject_short_intervals_and_non_utc_timezones() {
        let mut value = request("create_backup");
        value.timezone = "UTC".to_owned();
        value.target_ids.clear();
        value.backup_secret_ref = Some(SecretId::new());
        value.every_minutes = 59;
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "backup_interval_too_short"
        );
        value.every_minutes = 60;
        value.timezone = "Europe/Berlin".to_owned();
        assert_eq!(
            validate_schedule_fields(&value).unwrap_err().code,
            "invalid_backup_schedule_timezone"
        );
    }
}
