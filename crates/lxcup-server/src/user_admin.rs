use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::Utc;
use lxcup_persistence::{AuthAuditEvent, AuthAuditFilters, AuthUser, AuthUserRole};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::user_auth::hash_password;
use crate::{ApiEnvelope, ApiError, ApiState, AuthenticatedUser, envelope};

#[derive(Serialize)]
pub(crate) struct ManagedUserDto {
    id: Uuid,
    username: String,
    role: &'static str,
    must_change_password: bool,
    disabled: bool,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

impl From<AuthUser> for ManagedUserDto {
    fn from(user: AuthUser) -> Self {
        Self {
            id: user.id,
            username: user.username,
            role: role_name(user.role),
            must_change_password: user.must_change_password,
            disabled: user.disabled,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateUserRequest {
    username: String,
    role: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct UpdateUserRequest {
    role: String,
    disabled: bool,
}

#[derive(Deserialize)]
pub(crate) struct DeleteUserRequest {
    confirmed: bool,
}

#[derive(Deserialize)]
pub(crate) struct ResetPasswordRequest {
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct AuditQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    actor_username: Option<String>,
    action: Option<String>,
    resource: Option<String>,
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
}

#[derive(Serialize)]
pub(crate) struct UserAuditPageDto {
    events: Vec<UserAuditDto>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Serialize)]
pub(crate) struct UserAuditDto {
    id: Uuid,
    actor_user_id: Option<Uuid>,
    actor_username: String,
    actor_role: &'static str,
    action: String,
    resource_type: String,
    resource_id: Option<String>,
    request_id: Option<Uuid>,
    status_code: i16,
    details: serde_json::Value,
    created_at: chrono::DateTime<Utc>,
}

impl From<AuthAuditEvent> for UserAuditDto {
    fn from(event: AuthAuditEvent) -> Self {
        Self {
            id: event.id,
            actor_user_id: event.actor_user_id,
            actor_username: event.actor_username,
            actor_role: role_name(event.actor_role),
            action: event.action,
            resource_type: event.resource_type,
            resource_id: event.resource_id,
            request_id: event.request_id,
            status_code: event.status_code,
            details: event.details,
            created_at: event.created_at,
        }
    }
}

pub(crate) async fn list_users(
    State(state): State<ApiState>,
) -> Result<Json<ApiEnvelope<Vec<ManagedUserDto>>>, ApiError> {
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    let users = repositories
        .auth
        .list_users()
        .await
        .map_err(|_| ApiError::storage())?
        .into_iter()
        .map(ManagedUserDto::from)
        .collect();
    Ok(Json(envelope(users)))
}

pub(crate) async fn create_user(
    State(state): State<ApiState>,
    Json(request): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ManagedUserDto>>), ApiError> {
    let username = normalize_username(&request.username).ok_or_else(|| {
        ApiError::bad_request("invalid_username", "the username format is invalid")
    })?;
    let role = parse_role(&request.role)?;
    validate_password(&request.password)?;
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    if repositories
        .auth
        .user_by_username(&username)
        .await
        .map_err(|_| ApiError::storage())?
        .is_some()
    {
        return Err(ApiError::conflict(
            "username_exists",
            "that username is already in use",
        ));
    }
    let password = request.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| ApiError::storage())?
        .map_err(|_| ApiError::storage())?;
    let user = repositories
        .auth
        .create_user(&username, &password_hash, role, true)
        .await
        .map_err(|error| match error {
            lxcup_persistence::RepositoryError::UsernameExists => {
                ApiError::conflict("username_exists", "that username is already in use")
            }
            _ => ApiError::storage(),
        })?;
    Ok((
        StatusCode::CREATED,
        Json(envelope(ManagedUserDto::from(user))),
    ))
}

pub(crate) async fn update_user(
    State(state): State<ApiState>,
    Path(user_id): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<ApiEnvelope<ManagedUserDto>>, ApiError> {
    let user_id = parse_user_id(&user_id)?;
    let role = parse_role(&request.role)?;
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    let user = repositories
        .auth
        .update_user(user_id, role, request.disabled)
        .await
        .map_err(map_user_repository_error)?
        .ok_or_else(|| ApiError::not_found("user not found"))?;
    Ok(Json(envelope(ManagedUserDto::from(user))))
}

pub(crate) async fn delete_user(
    State(state): State<ApiState>,
    Path(user_id): Path<String>,
    Extension(actor): Extension<AuthenticatedUser>,
    Json(request): Json<DeleteUserRequest>,
) -> Result<StatusCode, ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "explicit confirmation is required",
        ));
    }
    let user_id = parse_user_id(&user_id)?;
    if user_id == actor.id {
        return Err(ApiError::conflict(
            "cannot_delete_current_user",
            "sign in with a different administrator before deleting this account",
        ));
    }
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    let deleted = repositories
        .auth
        .delete_user(user_id)
        .await
        .map_err(map_user_repository_error)?;
    if !deleted {
        return Err(ApiError::not_found("user not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn reset_user_password(
    State(state): State<ApiState>,
    Path(user_id): Path<String>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    validate_password(&request.password)?;
    let user_id = parse_user_id(&user_id)?;
    let password = request.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| ApiError::storage())?
        .map_err(|_| ApiError::storage())?;
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    let updated = repositories
        .auth
        .reset_user_password(user_id, &password_hash)
        .await
        .map_err(|_| ApiError::storage())?;
    if !updated {
        return Err(ApiError::not_found("active user not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_user_audit(
    State(state): State<ApiState>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<ApiEnvelope<UserAuditPageDto>>, ApiError> {
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    let page = repositories
        .auth
        .list_audit_events(&AuthAuditFilters {
            limit: query.limit.unwrap_or(50),
            offset: query.offset.unwrap_or(0),
            actor_username: query.actor_username,
            action: query.action,
            resource: query.resource,
            since: query.since,
            until: query.until,
        })
        .await
        .map_err(|_| ApiError::storage())?;
    Ok(Json(envelope(UserAuditPageDto {
        events: page.events.into_iter().map(UserAuditDto::from).collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    })))
}

fn parse_user_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .map_err(|_| ApiError::bad_request("invalid_user_id", "user id is invalid"))
}

fn normalize_username(username: &str) -> Option<String> {
    let username = username.trim();
    if username.is_empty()
        || username.len() > 64
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(username.to_ascii_lowercase())
}

fn parse_role(role: &str) -> Result<AuthUserRole, ApiError> {
    match role {
        "admin" => Ok(AuthUserRole::Admin),
        "user" => Ok(AuthUserRole::User),
        _ => Err(ApiError::bad_request(
            "invalid_role",
            "role must be admin or user",
        )),
    }
}

fn role_name(role: AuthUserRole) -> &'static str {
    match role {
        AuthUserRole::Admin => "admin",
        AuthUserRole::User => "user",
    }
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 12 || password.len() > 1024 || password.trim().is_empty() {
        return Err(ApiError::bad_request(
            "weak_password",
            "password must contain at least 12 characters",
        ));
    }
    Ok(())
}

fn map_user_repository_error(error: lxcup_persistence::RepositoryError) -> ApiError {
    match error {
        lxcup_persistence::RepositoryError::LastAdmin => ApiError::conflict(
            "last_admin_required",
            "the last active administrator cannot be removed, disabled, or demoted",
        ),
        _ => ApiError::storage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_normalization_trims_and_rejects_invalid_values() {
        assert_eq!(
            normalize_username("  Niklas.Admin-1_  "),
            Some("niklas.admin-1_".into())
        );
        assert_eq!(normalize_username("  "), None);
        assert_eq!(normalize_username(&"a".repeat(65)), None);
        assert_eq!(normalize_username("name with spaces"), None);
        assert_eq!(normalize_username("name/with/slash"), None);
        assert_eq!(normalize_username("ümlaut"), None);
    }

    #[test]
    fn user_ids_and_roles_are_parsed_from_the_supported_values() {
        let id = Uuid::new_v4();
        assert_eq!(parse_user_id(&id.to_string()).unwrap(), id);
        assert_eq!(
            parse_user_id("not-a-uuid").unwrap_err().code,
            "invalid_user_id"
        );
        assert!(matches!(parse_role("admin"), Ok(AuthUserRole::Admin)));
        assert!(matches!(parse_role("user"), Ok(AuthUserRole::User)));
        assert_eq!(parse_role("viewer").unwrap_err().code, "invalid_role");
        assert_eq!(role_name(AuthUserRole::Admin), "admin");
        assert_eq!(role_name(AuthUserRole::User), "user");
    }

    #[test]
    fn password_validation_enforces_length_and_non_whitespace_content() {
        assert!(validate_password("long-enough-password").is_ok());
        assert_eq!(
            validate_password("short").unwrap_err().code,
            "weak_password"
        );
        assert_eq!(
            validate_password(&"a".repeat(1025)).unwrap_err().code,
            "weak_password"
        );
        assert_eq!(
            validate_password("            ").unwrap_err().code,
            "weak_password"
        );
    }

    #[test]
    fn repository_errors_keep_last_admin_conflict_specific() {
        assert_eq!(
            map_user_repository_error(lxcup_persistence::RepositoryError::LastAdmin).code,
            "last_admin_required"
        );
        assert_eq!(
            map_user_repository_error(lxcup_persistence::RepositoryError::UsernameExists).code,
            "storage_error"
        );
    }
}
