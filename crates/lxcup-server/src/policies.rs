use super::{ApiEnvelope, ApiError, ApiState, envelope, require_permission};
use axum::{
    Json,
    extract::{Json as JsonBody, Path, State},
};
use lxcup_core::{ActorRole, Permission, TargetId, UpdatePolicy, UpdateRisk};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateUpdatePolicyRequest {
    pub id: String,
    pub allowed_targets: Vec<TargetId>,
    #[serde(default)]
    pub allowed_packages: Vec<String>,
    pub maintenance_start_minute: u16,
    pub maintenance_end_minute: u16,
    pub timezone: String,
    pub maximum_risk: UpdateRisk,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DeleteUpdatePolicyRequest {
    pub confirmed: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) const STANDARD_UPDATE_POLICY_ID: &str = "lxcup-standard-all-packages";

pub(crate) fn default_update_policy(mut target_ids: Vec<TargetId>) -> UpdatePolicy {
    target_ids.sort_by_key(|target_id| target_id.as_uuid());
    target_ids.dedup();
    UpdatePolicy {
        id: STANDARD_UPDATE_POLICY_ID.to_owned(),
        allowed_targets: target_ids,
        allowed_packages: Vec::new(),
        maintenance_start_minute: 0,
        maintenance_end_minute: 23 * 60 + 59,
        timezone: "UTC".to_owned(),
        maximum_risk: UpdateRisk::High,
        enabled: true,
    }
}

pub(super) async fn list_update_policies(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<UpdatePolicy>>>, ApiError> {
    // Reconcile defaults here too, so an earlier startup/database hiccup does
    // not leave existing resources without their standard policy indefinitely.
    state.ensure_default_update_policies().await?;
    let policies = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .update_policies
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.update_policies.clone()
    };
    Ok(Json(envelope(policies)))
}

pub(super) async fn create_update_policy(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    JsonBody(request): JsonBody<CreateUpdatePolicyRequest>,
) -> Result<(axum::http::StatusCode, Json<ApiEnvelope<UpdatePolicy>>), ApiError> {
    require_permission(actor_role, Permission::Configure)?;
    if request.id == STANDARD_UPDATE_POLICY_ID {
        return Err(ApiError::bad_request(
            "reserved_policy_id",
            "this policy id is reserved for the system default",
        ));
    }
    if request.id.trim().is_empty() || request.id.len() > 128 {
        return Err(ApiError::bad_request(
            "invalid_policy",
            "policy id is required and bounded",
        ));
    }
    if request.timezone.trim().is_empty() || request.timezone.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_timezone",
            "timezone is required and bounded",
        ));
    }
    if request.timezone != "UTC" {
        return Err(ApiError::bad_request(
            "unsupported_timezone",
            "update policy windows currently use UTC",
        ));
    }
    if request.maintenance_start_minute >= 24 * 60 || request.maintenance_end_minute >= 24 * 60 {
        return Err(ApiError::bad_request(
            "invalid_window",
            "maintenance minutes must be within a day",
        ));
    }
    if request.allowed_targets.is_empty() {
        return Err(ApiError::bad_request(
            "target_required",
            "at least one target is required",
        ));
    }
    if request.allowed_packages.len() > 500
        || request.allowed_packages.iter().any(|package| {
            package.trim().is_empty()
                || package.len() > 100
                || package.chars().any(char::is_whitespace)
                || package.chars().any(|character| {
                    !(character.is_ascii_alphanumeric() || "+._:-".contains(character))
                })
        })
    {
        return Err(ApiError::bad_request(
            "invalid_package_group",
            "allowed package names must be bounded safe package identifiers",
        ));
    }
    let mut store = state.store.write().await;
    if store
        .update_policies
        .iter()
        .any(|policy| policy.id == request.id)
    {
        return Err(ApiError::conflict(
            "policy_exists",
            "policy id already exists",
        ));
    }
    if request
        .allowed_targets
        .iter()
        .any(|id| !store.targets.iter().any(|target| target.id == *id))
    {
        return Err(ApiError::not_found("policy target not found"));
    }
    let policy = UpdatePolicy {
        id: request.id,
        allowed_targets: request.allowed_targets,
        allowed_packages: request.allowed_packages,
        maintenance_start_minute: request.maintenance_start_minute,
        maintenance_end_minute: request.maintenance_end_minute,
        timezone: request.timezone,
        maximum_risk: request.maximum_risk,
        enabled: request.enabled,
    };
    store.update_policies.push(policy.clone());
    drop(store);
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .update_policies
            .save(&policy)
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok((axum::http::StatusCode::CREATED, Json(envelope(policy))))
}

pub(super) async fn delete_update_policy(
    State(state): State<ApiState>,
    axum::Extension(actor_role): axum::Extension<ActorRole>,
    Path(policy_id): Path<String>,
    JsonBody(request): JsonBody<DeleteUpdatePolicyRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_permission(actor_role, Permission::Destructive)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "deleting an update policy requires explicit confirmation",
        ));
    }
    if policy_id == STANDARD_UPDATE_POLICY_ID {
        return Err(ApiError::bad_request(
            "system_policy_protected",
            "the system-wide standard update policy cannot be deleted",
        ));
    }

    let schedules = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .schedules
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.store.read().await.schedules.clone()
    };
    if schedules.iter().any(|schedule| {
        schedule.enabled && schedule.policy_id.as_deref() == Some(policy_id.as_str())
    }) {
        return Err(ApiError::conflict(
            "policy_in_use",
            "disable recurring schedules that reference this policy before deleting it",
        ));
    }

    let deleted = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .update_policies
            .delete(&policy_id)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        let mut store = state.store.write().await;
        let previous_count = store.update_policies.len();
        store
            .update_policies
            .retain(|policy| policy.id != policy_id);
        previous_count != store.update_policies.len()
    };
    if !deleted {
        return Err(ApiError::not_found("update policy not found"));
    }

    state
        .store
        .write()
        .await
        .update_policies
        .retain(|policy| policy.id != policy_id);
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .audit_events
            .append(&lxcup_persistence::AuditEvent {
                id: uuid::Uuid::new_v4(),
                node_id: None,
                container_id: None,
                plan_id: None,
                execution_id: None,
                event_type: "update_policy.deleted".to_owned(),
                details: serde_json::json!({
                    "policy_id": policy_id,
                    "role": format!("{actor_role:?}").to_lowercase(),
                }),
                created_at: chrono::Utc::now(),
            })
            .await
            .map_err(|_| ApiError::storage())?;
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Extension, http::StatusCode};
    use lxcup_core::{Target, TargetKind, TargetTransport};

    fn request(target_id: TargetId) -> CreateUpdatePolicyRequest {
        CreateUpdatePolicyRequest {
            id: "nightly-patches".to_owned(),
            allowed_targets: vec![target_id],
            allowed_packages: vec!["curl".to_owned(), "openssl".to_owned()],
            maintenance_start_minute: 60,
            maintenance_end_minute: 120,
            timezone: "UTC".to_owned(),
            maximum_risk: UpdateRisk::High,
            enabled: true,
        }
    }

    async fn state_with_target() -> (ApiState, TargetId) {
        let state = ApiState::new();
        let target = Target::new(
            "policy-target",
            TargetKind::LinuxServer,
            "192.0.2.44",
            TargetTransport::Ssh,
            lxcup_core::SecretId::new(),
            lxcup_core::SecretId::new(),
        )
        .unwrap();
        let target_id = target.id;
        state.store.write().await.targets.push(target);
        (state, target_id)
    }

    #[test]
    fn default_policy_allows_all_packages_and_the_full_utc_day() {
        let target_id = TargetId::new();
        let policy = default_update_policy(vec![target_id]);

        assert_eq!(policy.allowed_targets, vec![target_id]);
        assert!(policy.allowed_packages.is_empty());
        assert_eq!(policy.maintenance_start_minute, 0);
        assert_eq!(policy.maintenance_end_minute, 23 * 60 + 59);
        assert_eq!(policy.timezone, "UTC");
        assert_eq!(policy.maximum_risk, UpdateRisk::High);
        assert!(policy.enabled);
    }

    #[tokio::test]
    async fn default_policy_backfill_is_idempotent_and_preserves_existing_policy() {
        let (state, target_id) = state_with_target().await;
        let mut existing = default_update_policy(vec![target_id]);
        existing.maximum_risk = UpdateRisk::Low;
        state
            .store
            .write()
            .await
            .update_policies
            .push(existing.clone());

        assert_eq!(state.ensure_default_update_policies().await.unwrap(), 0);
        let policies = state.store.read().await.update_policies.clone();
        assert_eq!(policies, vec![existing]);
    }

    #[tokio::test]
    async fn default_policy_backfill_creates_one_policy_for_each_missing_target() {
        let (state, first_id) = state_with_target().await;
        let second = lxcup_core::Target::new(
            "second-policy-target",
            TargetKind::LinuxServer,
            "192.0.2.45",
            TargetTransport::Ssh,
            lxcup_core::SecretId::new(),
            lxcup_core::SecretId::new(),
        )
        .unwrap();
        let second_id = second.id;
        state.store.write().await.targets.push(second);

        assert_eq!(state.ensure_default_update_policies().await.unwrap(), 1);
        assert_eq!(state.ensure_default_update_policies().await.unwrap(), 0);
        let policies = state.store.read().await.update_policies.clone();
        assert_eq!(policies.len(), 1);
        let mut expected_target_ids = vec![first_id, second_id];
        expected_target_ids.sort_by_key(|target_id| target_id.as_uuid());
        assert_eq!(policies[0].allowed_targets, expected_target_ids);
    }

    #[tokio::test]
    async fn onboarding_extends_the_shared_policy_without_creating_another() {
        let (state, first_id) = state_with_target().await;
        assert_eq!(state.ensure_default_update_policies().await.unwrap(), 1);

        let second = lxcup_core::Target::new(
            "newly-onboarded-target",
            TargetKind::LinuxServer,
            "192.0.2.46",
            TargetTransport::Ssh,
            lxcup_core::SecretId::new(),
            lxcup_core::SecretId::new(),
        )
        .unwrap();
        let second_id = second.id;
        state.store.write().await.targets.push(second);

        assert_eq!(state.ensure_default_update_policies().await.unwrap(), 1);
        let policies = state.store.read().await.update_policies.clone();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].id, STANDARD_UPDATE_POLICY_ID);
        assert!(policies[0].allowed_targets.contains(&first_id));
        assert!(policies[0].allowed_targets.contains(&second_id));
    }

    #[tokio::test]
    async fn listing_policies_backfills_defaults_for_existing_targets() {
        let (state, target_id) = state_with_target().await;

        let listed = list_update_policies(State(state.clone())).await.unwrap();

        assert_eq!(listed.0.data, vec![default_update_policy(vec![target_id])]);
        assert_eq!(state.store.read().await.update_policies.len(), 1);
    }

    async fn create(
        state: ApiState,
        role: ActorRole,
        request: CreateUpdatePolicyRequest,
    ) -> Result<(StatusCode, Json<ApiEnvelope<UpdatePolicy>>), ApiError> {
        create_update_policy(State(state), Extension(role), JsonBody(request)).await
    }

    async fn delete(
        state: ApiState,
        role: ActorRole,
        policy_id: &str,
        confirmed: bool,
    ) -> Result<StatusCode, ApiError> {
        delete_update_policy(
            State(state),
            Extension(role),
            Path(policy_id.to_owned()),
            JsonBody(DeleteUpdatePolicyRequest { confirmed }),
        )
        .await
    }

    #[test]
    fn omitted_enabled_flag_defaults_to_true() {
        assert!(default_true());
    }

    #[tokio::test]
    async fn create_and_list_policy_persists_a_valid_policy_in_memory() {
        let (state, target_id) = state_with_target().await;
        let (status, created) = create(state.clone(), ActorRole::Admin, request(target_id))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created.0.data.id, "nightly-patches");
        assert_eq!(created.0.data.allowed_targets, vec![target_id]);
        assert_eq!(
            created.0.data.allowed_packages,
            vec!["curl".to_owned(), "openssl".to_owned()]
        );
        assert!(created.0.data.enabled);

        let listed = list_update_policies(State(state)).await.unwrap();
        assert!(listed.0.data.contains(&created.0.data));
        assert_eq!(listed.0.data.len(), 2);
    }

    #[tokio::test]
    async fn create_policy_rejects_permission_and_invalid_policy_fields() {
        let (state, target_id) = state_with_target().await;
        let denied = create(state.clone(), ActorRole::Viewer, request(target_id))
            .await
            .unwrap_err();
        assert_eq!(denied.code, "permission_denied");

        let cases: [(&str, fn(&mut CreateUpdatePolicyRequest), &str); 12] = [
            (
                "empty-id",
                |request: &mut CreateUpdatePolicyRequest| request.id = "  ".to_owned(),
                "invalid_policy",
            ),
            (
                "long-id",
                |request: &mut CreateUpdatePolicyRequest| request.id = "x".repeat(129),
                "invalid_policy",
            ),
            (
                "empty-timezone",
                |request: &mut CreateUpdatePolicyRequest| request.timezone = " ".to_owned(),
                "invalid_timezone",
            ),
            (
                "unsupported-timezone",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.timezone = "Europe/Berlin".to_owned()
                },
                "unsupported_timezone",
            ),
            (
                "start-out-of-range",
                |request: &mut CreateUpdatePolicyRequest| request.maintenance_start_minute = 1440,
                "invalid_window",
            ),
            (
                "end-out-of-range",
                |request: &mut CreateUpdatePolicyRequest| request.maintenance_end_minute = 1440,
                "invalid_window",
            ),
            (
                "no-targets",
                |request: &mut CreateUpdatePolicyRequest| request.allowed_targets.clear(),
                "target_required",
            ),
            (
                "empty-package",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.allowed_packages = vec![" ".to_owned()]
                },
                "invalid_package_group",
            ),
            (
                "unsafe-package",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.allowed_packages = vec!["curl;reboot".to_owned()]
                },
                "invalid_package_group",
            ),
            (
                "long-package",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.allowed_packages = vec!["x".repeat(101)]
                },
                "invalid_package_group",
            ),
            (
                "too-many-packages",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.allowed_packages = vec!["curl".to_owned(); 501]
                },
                "invalid_package_group",
            ),
            (
                "unknown-target",
                |request: &mut CreateUpdatePolicyRequest| {
                    request.allowed_targets = vec![TargetId::from_uuid(uuid::Uuid::new_v4())]
                },
                "not_found",
            ),
        ];

        for (case, mutate, expected_code) in cases {
            let mut invalid = request(target_id);
            mutate(&mut invalid);
            let error = create(state.clone(), ActorRole::Admin, invalid)
                .await
                .unwrap_err();
            assert_eq!(error.code, expected_code, "case {case}");
        }
    }

    #[tokio::test]
    async fn create_policy_rejects_duplicate_ids() {
        let (state, target_id) = state_with_target().await;
        let created = create(state.clone(), ActorRole::Admin, request(target_id))
            .await
            .unwrap();
        assert_eq!(created.0, StatusCode::CREATED);

        let duplicate = create(state, ActorRole::Admin, request(target_id))
            .await
            .unwrap_err();
        assert_eq!(duplicate.code, "policy_exists");
    }

    #[tokio::test]
    async fn create_policy_rejects_the_reserved_standard_policy_id() {
        let (state, target_id) = state_with_target().await;
        let mut request = request(target_id);
        request.id = STANDARD_UPDATE_POLICY_ID.to_owned();

        let error = create(state, ActorRole::Admin, request).await.unwrap_err();

        assert_eq!(error.code, "reserved_policy_id");
    }

    #[tokio::test]
    async fn delete_policy_requires_destructive_permission_and_confirmation() {
        let (state, target_id) = state_with_target().await;
        let _created = create(state.clone(), ActorRole::Admin, request(target_id))
            .await
            .unwrap();

        let denied = delete(state.clone(), ActorRole::Operator, "nightly-patches", true)
            .await
            .unwrap_err();
        assert_eq!(denied.code, "permission_denied");

        let unconfirmed = delete(state.clone(), ActorRole::Admin, "nightly-patches", false)
            .await
            .unwrap_err();
        assert_eq!(unconfirmed.code, "confirmation_required");
        assert_eq!(state.store.read().await.update_policies.len(), 1);

        assert_eq!(
            delete(state.clone(), ActorRole::Admin, "nightly-patches", true)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(state.store.read().await.update_policies.is_empty());
        assert_eq!(
            delete(state, ActorRole::Admin, "nightly-patches", true)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn delete_policy_protects_the_system_wide_standard_policy() {
        let (state, _) = state_with_target().await;
        state.ensure_default_update_policies().await.unwrap();

        let error = delete(
            state.clone(),
            ActorRole::Admin,
            STANDARD_UPDATE_POLICY_ID,
            true,
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, "system_policy_protected");
        assert_eq!(state.store.read().await.update_policies.len(), 1);
    }

    #[tokio::test]
    async fn delete_policy_rejects_policies_used_by_an_enabled_schedule() {
        let (state, target_id) = state_with_target().await;
        let _created = create(state.clone(), ActorRole::Admin, request(target_id))
            .await
            .unwrap();
        state
            .store
            .write()
            .await
            .schedules
            .push(lxcup_core::JobSchedule {
                id: "nightly-update".to_owned(),
                operation: "update_packages".to_owned(),
                timezone: "UTC".to_owned(),
                target_ids: vec![target_id],
                frequency: lxcup_core::ScheduleFrequency::EveryMinutes(60),
                enabled: true,
                threshold: None,
                policy_id: Some("nightly-patches".to_owned()),
                last_run_at: None,
                next_run_at: chrono::Utc::now(),
                last_error: None,
            });

        let error = delete(state.clone(), ActorRole::Admin, "nightly-patches", true)
            .await
            .unwrap_err();

        assert_eq!(error.code, "policy_in_use");
        assert_eq!(state.store.read().await.update_policies.len(), 1);
    }
}
