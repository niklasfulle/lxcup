use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use chrono::{Duration, Utc};
use lxcup_persistence::{AuthUser, AuthUserRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{ApiEnvelope, ApiError, ApiState, envelope};

const PASSWORD_HASH_ROUNDS: u32 = 600_000;
const SESSION_LIFETIME_HOURS: i64 = 8;
const SESSION_LIFETIME_SECONDS: u64 = 8 * 60 * 60;
pub(crate) const SESSION_COOKIE_NAME: &str = "lxcup_session";
pub(crate) const CSRF_COOKIE_NAME: &str = "lxcup_csrf";

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedUser {
    pub id: Uuid,
    pub username: String,
    pub role: AuthUserRole,
    pub must_change_password: bool,
    pub token_hash: String,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Deserialize)]
pub(crate) struct AuthLoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub(crate) struct AuthStatusResponse {
    enabled: bool,
}

#[derive(Serialize)]
pub(crate) struct AuthLoginResponse {
    user: AuthUserResponse,
    expires_in_seconds: u64,
}

#[derive(Serialize)]
pub(crate) struct AuthUserResponse {
    id: Uuid,
    username: String,
    role: &'static str,
    must_change_password: bool,
}

impl From<&AuthUser> for AuthUserResponse {
    fn from(user: &AuthUser) -> Self {
        Self {
            id: user.id,
            username: user.username.clone(),
            role: role_name(user.role),
            must_change_password: user.must_change_password,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ChangePasswordRequest {
    current_password: Option<String>,
    new_password: String,
}

pub(crate) async fn auth_status(
    State(state): State<ApiState>,
) -> Json<ApiEnvelope<AuthStatusResponse>> {
    Json(envelope(AuthStatusResponse {
        enabled: state.account_auth_enabled,
    }))
}

pub(crate) async fn auth_login(
    State(state): State<ApiState>,
    Extension(request_id): Extension<Uuid>,
    Json(request): Json<AuthLoginRequest>,
) -> Result<(HeaderMap, Json<ApiEnvelope<AuthLoginResponse>>), ApiError> {
    if !state.account_auth_enabled {
        return Err(ApiError::not_found("account authentication is disabled"));
    }
    if request.password.len() > 1024 {
        return Err(ApiError::unauthorized());
    }
    let Some(repositories) = state.repositories.as_ref() else {
        return Err(ApiError::storage());
    };
    let Some(username) = normalize_username(&request.username) else {
        let password = request.password;
        let _ =
            tokio::task::spawn_blocking(move || verify_password(&password, DUMMY_PASSWORD_HASH))
                .await;
        record_failed_login(&repositories.auth, None, request_id).await?;
        return Err(ApiError::unauthorized());
    };
    let user = repositories
        .auth
        .user_by_username(&username)
        .await
        .map_err(|_| ApiError::storage())?;
    let Some(user) = user.filter(|user| !user.disabled) else {
        let password = request.password;
        let _ =
            tokio::task::spawn_blocking(move || verify_password(&password, DUMMY_PASSWORD_HASH))
                .await;
        record_failed_login(&repositories.auth, Some(&username), request_id).await?;
        return Err(ApiError::unauthorized());
    };
    let password = request.password;
    let password_hash = user.password_hash.clone();
    if !tokio::task::spawn_blocking(move || verify_password(&password, &password_hash))
        .await
        .map_err(|_| ApiError::storage())?
    {
        record_failed_login(&repositories.auth, Some(&username), request_id).await?;
        return Err(ApiError::unauthorized());
    }

    let access_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let csrf_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash = hash_session_token(&access_token);
    let cookie_headers = session_cookie_headers(&access_token, &csrf_token, state.secure_cookies)?;
    let expires_at = Utc::now() + Duration::hours(SESSION_LIFETIME_HOURS);
    let audit_event = lxcup_persistence::AuthAuditEvent {
        id: Uuid::new_v4(),
        actor_user_id: Some(user.id),
        actor_username: user.username.clone(),
        actor_role: user.role,
        action: "auth.login".to_owned(),
        resource_type: "session".to_owned(),
        resource_id: None,
        request_id: Some(request_id),
        status_code: StatusCode::PROCESSING.as_u16() as i16,
        details: serde_json::json!({"state": "pending"}),
        created_at: Utc::now(),
    };
    repositories
        .auth
        .append_audit_event(&audit_event)
        .await
        .map_err(|_| ApiError::storage())?;
    if repositories
        .auth
        .save_session(user.id, &token_hash, expires_at)
        .await
        .is_err()
    {
        let _ = repositories
            .auth
            .update_audit_event_status(
                audit_event.id,
                StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i16,
                "failed",
            )
            .await;
        return Err(ApiError::storage());
    }
    let audit_updated = repositories
        .auth
        .update_audit_event_status(audit_event.id, StatusCode::OK.as_u16() as i16, "completed")
        .await
        .unwrap_or(false);
    if !audit_updated {
        let _ = repositories.auth.revoke_session(&token_hash).await;
        return Err(ApiError::storage());
    }
    Ok((
        cookie_headers,
        Json(envelope(AuthLoginResponse {
            user: AuthUserResponse::from(&user),
            expires_in_seconds: SESSION_LIFETIME_SECONDS,
        })),
    ))
}

pub(crate) fn session_cookie_headers(
    session_token: &str,
    csrf_token: &str,
    secure: bool,
) -> Result<HeaderMap, ApiError> {
    let secure_attribute = if secure { "; Secure" } else { "" };
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE_NAME}={session_token}; Path=/api/v1; Max-Age={SESSION_LIFETIME_SECONDS}; HttpOnly; SameSite=Strict{secure_attribute}"
        ))
        .map_err(|_| ApiError::storage())?,
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{CSRF_COOKIE_NAME}={csrf_token}; Path=/; Max-Age={SESSION_LIFETIME_SECONDS}; SameSite=Strict{secure_attribute}"
        ))
        .map_err(|_| ApiError::storage())?,
    );
    Ok(headers)
}

pub(crate) fn cleared_session_cookie_headers(secure: bool) -> Result<HeaderMap, ApiError> {
    let secure_attribute = if secure { "; Secure" } else { "" };
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE_NAME}=; Path=/api/v1; Max-Age=0; HttpOnly; SameSite=Strict{secure_attribute}"
        ))
        .map_err(|_| ApiError::storage())?,
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{CSRF_COOKIE_NAME}=; Path=/; Max-Age=0; SameSite=Strict{secure_attribute}"
        ))
        .map_err(|_| ApiError::storage())?,
    );
    Ok(headers)
}

async fn record_failed_login(
    repository: &lxcup_persistence::AuthRepository,
    username: Option<&str>,
    request_id: Uuid,
) -> Result<(), ApiError> {
    let event = lxcup_persistence::AuthAuditEvent {
        id: Uuid::new_v4(),
        actor_user_id: None,
        actor_username: username.unwrap_or("unknown").to_owned(),
        actor_role: AuthUserRole::User,
        action: "auth.login_failed".to_owned(),
        resource_type: "session".to_owned(),
        resource_id: None,
        request_id: Some(request_id),
        status_code: StatusCode::UNAUTHORIZED.as_u16() as i16,
        details: serde_json::json!({}),
        created_at: Utc::now(),
    };
    repository
        .append_audit_event(&event)
        .await
        .map_err(|_| ApiError::storage())
}

pub(crate) async fn change_password(
    State(state): State<ApiState>,
    Extension(actor): Extension<AuthenticatedUser>,
    Json(request): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    validate_password(&request.new_password)?;
    let repositories = state.repositories.as_ref().ok_or_else(ApiError::storage)?;
    if !actor.must_change_password {
        let current_password = request.current_password.as_deref().ok_or_else(|| {
            ApiError::bad_request(
                "current_password_required",
                "the current password is required",
            )
        })?;
        if current_password.len() > 1024 {
            return Err(ApiError::bad_request(
                "invalid_current_password",
                "the current password is invalid",
            ));
        }
        let current_user = repositories
            .auth
            .user_by_id(actor.id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(ApiError::unauthorized)?;
        let current_password = current_password.to_owned();
        let password_hash = current_user.password_hash;
        if !tokio::task::spawn_blocking(move || verify_password(&current_password, &password_hash))
            .await
            .map_err(|_| ApiError::storage())?
        {
            return Err(ApiError::bad_request(
                "invalid_current_password",
                "the current password is incorrect",
            ));
        }
    }
    let new_password = request.new_password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&new_password))
        .await
        .map_err(|_| ApiError::storage())?
        .map_err(|_| ApiError::storage())?;
    let updated = repositories
        .auth
        .change_password(actor.id, &password_hash, &actor.token_hash)
        .await
        .map_err(|_| ApiError::storage())?;
    if !updated {
        return Err(ApiError::unauthorized());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn hash_password(password: &str) -> Result<String, ()> {
    let salt = Uuid::new_v4().as_bytes().to_owned();
    let derived = pbkdf2_sha256(password.as_bytes(), &salt, PASSWORD_HASH_ROUNDS)?;
    Ok(format!(
        "pbkdf2-sha256${PASSWORD_HASH_ROUNDS}${}${}",
        encode_hex(&salt),
        encode_hex(&derived)
    ))
}

pub(crate) fn hash_session_token(token: &str) -> String {
    encode_hex(&Sha256::digest(token.as_bytes()))
}

fn verify_password(password: &str, encoded: &str) -> bool {
    let mut fields = encoded.split('$');
    let (Some("pbkdf2-sha256"), Some(rounds), Some(salt), Some(expected), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let Ok(rounds) = rounds.parse::<u32>() else {
        return false;
    };
    if !(100_000..=2_000_000).contains(&rounds) {
        return false;
    }
    let (Some(salt), Some(expected)) = (decode_hex(salt), decode_hex(expected)) else {
        return false;
    };
    if salt.len() != 16 || expected.len() != 32 {
        return false;
    }
    let Ok(actual) = pbkdf2_sha256(password.as_bytes(), &salt, rounds) else {
        return false;
    };
    bool::from(actual.as_slice().ct_eq(expected.as_slice()))
}

fn pbkdf2_sha256(password: &[u8], salt: &[u8], rounds: u32) -> Result<[u8; 32], ()> {
    let rounds = NonZeroU32::new(rounds).ok_or(())?;
    let mut result = [0; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        password,
        &mut result,
    );
    Ok(result)
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 12 {
        return Err(ApiError::bad_request(
            "weak_password",
            "the new password must contain at least 12 characters",
        ));
    }
    if password.len() > 1024 || password.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_password",
            "the new password is invalid",
        ));
    }
    Ok(())
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

fn role_name(role: AuthUserRole) -> &'static str {
    match role {
        AuthUserRole::Admin => "admin",
        AuthUserRole::User => "user",
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

const DUMMY_PASSWORD_HASH: &str = "pbkdf2-sha256$600000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

#[cfg(test)]
mod tests {
    use super::{
        decode_hex, hash_password, normalize_username, pbkdf2_sha256, session_cookie_headers,
        validate_password, verify_password,
    };
    use axum::http::header;

    #[test]
    fn pbkdf2_sha256_matches_the_standard_single_iteration_vector() {
        let actual = pbkdf2_sha256(b"password", b"salt", 1).unwrap();
        assert_eq!(
            super::encode_hex(&actual),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
    }

    #[test]
    fn username_normalization_is_local_and_rejects_email_or_injection_syntax() {
        assert_eq!(
            normalize_username("  Niklas_1 "),
            Some("niklas_1".to_owned())
        );
        assert_eq!(normalize_username("name@example.org"), None);
        assert_eq!(normalize_username("user;drop"), None);
        assert_eq!(decode_hex("a0ff"), Some(vec![160, 255]));
        assert_eq!(decode_hex("xyz"), None);
    }

    #[test]
    fn password_policy_requires_a_long_nonblank_password() {
        assert!(validate_password("long-enough-password").is_ok());
        assert!(validate_password("short").is_err());
        assert!(validate_password("            ").is_err());
        assert!(validate_password(&"a".repeat(1025)).is_err());
    }

    #[test]
    fn password_hash_uses_a_salt_and_verifies_without_exposing_the_password() {
        let encoded = hash_password("a-long-test-password").unwrap();
        assert!(encoded.starts_with("pbkdf2-sha256$600000$"));
        assert!(!encoded.contains("a-long-test-password"));
        assert!(verify_password("a-long-test-password", &encoded));
        assert!(!verify_password("a-different-password", &encoded));
    }

    #[test]
    fn session_cookie_is_http_only_and_csrf_cookie_is_separate() {
        let headers = session_cookie_headers("session-secret", "csrf-secret", true).unwrap();
        let cookies = headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();

        let session = cookies
            .iter()
            .find(|cookie| cookie.starts_with("lxcup_session="))
            .unwrap();
        assert!(session.contains("HttpOnly"));
        assert!(session.contains("Secure"));
        assert!(session.contains("SameSite=Strict"));
        assert!(session.contains("Path=/api/v1"));
        assert!(session.contains("Max-Age=28800"));

        let csrf = cookies
            .iter()
            .find(|cookie| cookie.starts_with("lxcup_csrf="))
            .unwrap();
        assert!(!csrf.contains("HttpOnly"));
        assert!(csrf.contains("Secure"));
        assert!(csrf.contains("SameSite=Strict"));
        assert!(csrf.contains("Path=/"));
        assert!(csrf.contains("Max-Age=28800"));
    }
}
