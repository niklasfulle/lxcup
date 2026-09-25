use super::{ApiEnvelope, ApiError, ApiState, envelope, require_permission};
use axum::{
    Json,
    extract::{Json as JsonBody, State},
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

fn default_true() -> bool {
    true
}

pub(super) async fn list_update_policies(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<UpdatePolicy>>>, ApiError> {
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

    async fn create(
        state: ApiState,
        role: ActorRole,
        request: CreateUpdatePolicyRequest,
    ) -> Result<(StatusCode, Json<ApiEnvelope<UpdatePolicy>>), ApiError> {
        create_update_policy(State(state), Extension(role), JsonBody(request)).await
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
        assert_eq!(listed.0.data, vec![created.0.data.clone()]);
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
}
