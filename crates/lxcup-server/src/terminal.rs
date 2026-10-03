use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Extension,
    extract::{Path, State, ws::WebSocketUpgrade},
    http::{HeaderMap, header, uri::Authority},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use lxcup_core::{
    SecretKind, SecretScope, SecretValue, Target, TargetId, TargetKind, TargetState,
    TargetTransport,
};
use lxcup_persistence::{AuthAuditEvent, AuthUserRole};
use lxcup_secrets::{SecretStatus, StoredSecretMetadata};
use uuid::Uuid;

use crate::{ApiError, ApiState, AuthenticatedUser};

mod terminal_ssh;

const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_SESSIONS_PER_USER: usize = 2;
const MAX_SESSIONS_PER_TARGET: usize = 1;

#[derive(Clone, Copy)]
struct ActiveTerminalSession {
    user_id: Uuid,
    target_id: TargetId,
}

#[derive(Default)]
pub(crate) struct TerminalSessionRegistry {
    sessions: Mutex<HashMap<Uuid, ActiveTerminalSession>>,
}

struct TerminalSessionLease {
    registry: Arc<TerminalSessionRegistry>,
    session_id: Uuid,
}

impl Drop for TerminalSessionLease {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.registry.sessions.lock() {
            sessions.remove(&self.session_id);
        }
    }
}

struct TerminalConnection {
    actor: AuthenticatedUser,
    target_name: String,
    address: String,
    username: String,
    credential_kind: SecretKind,
    credential: SecretValue,
    known_hosts: SecretValue,
}

pub(crate) async fn terminal_session(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
    actor: Option<Extension<AuthenticatedUser>>,
    Extension(request_id): Extension<Uuid>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let Some(Extension(actor)) = actor else {
        return Err(ApiError::unauthorized());
    };
    ensure_terminal_admin(state.account_auth_enabled, &actor)?;
    validate_websocket_origin(&headers)?;
    let target_id = Uuid::parse_str(&target_id)
        .map(TargetId::from_uuid)
        .map_err(|_| {
            ApiError::bad_request("invalid_target_id", "the selected resource is invalid")
        })?;
    let target = find_terminal_target(&state, target_id).await?;
    let connection = load_terminal_connection(&state, actor, target).await?;
    let session_id = Uuid::new_v4();
    let lease = reserve_terminal_session(
        state.terminal_sessions.clone(),
        session_id,
        connection.actor.id,
        target_id,
    )?;
    record_terminal_audit(
        &state,
        &connection.actor,
        target_id,
        Some(request_id),
        "terminal.started",
        101,
        serde_json::json!({"state": "started", "target_name": connection.target_name}),
    )
    .await?;

    let lease_actor = connection.actor.clone();
    let upgrade = websocket
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let outcome = terminal_ssh::run_session(socket, state.clone(), connection).await;
            if let Err(reason) = outcome {
                tracing::warn!(target_id = %target_id.as_uuid(), session_id = %session_id, error = reason, "admin terminal session ended with an error");
            }
            let state_name = match outcome {
                Ok(()) => "closed",
                Err("idle_timeout") => "idle_timeout",
                Err("maximum_duration") => "maximum_duration",
                Err(_) => "failed",
            };
            let status = if state_name == "failed" { 502 } else { 200 };
            if let Err(error) = record_terminal_audit(
                &state,
                &lease_actor,
                target_id,
                None,
                "terminal.ended",
                status,
                serde_json::json!({"state": state_name}),
            )
            .await
            {
                tracing::error!(target_id = %target_id.as_uuid(), session_id = %session_id, error = ?error, "could not record terminal session end");
            }
            drop(lease);
        });
    Ok(upgrade.into_response())
}

fn ensure_terminal_admin(
    account_auth_enabled: bool,
    actor: &AuthenticatedUser,
) -> Result<(), ApiError> {
    if account_auth_enabled && actor.role == AuthUserRole::Admin && !actor.must_change_password {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "terminal_admin_required",
        "an active administrator account is required for terminal access",
    ))
}

async fn find_terminal_target(state: &ApiState, target_id: TargetId) -> Result<Target, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        return repositories
            .targets
            .find_by_id(target_id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("resource not found"));
    }
    state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("resource not found"))
}

async fn load_terminal_connection(
    state: &ApiState,
    actor: AuthenticatedUser,
    target: Target,
) -> Result<TerminalConnection, ApiError> {
    if target.transport != TargetTransport::Ssh
        || !matches!(target.kind, TargetKind::Lxc | TargetKind::LinuxServer)
        || target.state != TargetState::Managed
    {
        return Err(ApiError::forbidden(
            "terminal_resource_unsupported",
            "terminal access is available only for enabled LXC and Linux server resources",
        ));
    }
    let username = target
        .ssh_user
        .as_deref()
        .filter(|username| valid_ssh_username(username))
        .ok_or_else(|| {
            ApiError::bad_request(
                "terminal_ssh_profile_invalid",
                "the resource SSH profile is incomplete or invalid",
            )
        })?
        .to_owned();
    if !valid_ssh_host(&target.address) {
        return Err(ApiError::bad_request(
            "terminal_ssh_profile_invalid",
            "the resource SSH profile is incomplete or invalid",
        ));
    }

    let credential_metadata = active_secret_metadata(state, target.credential_secret_ref)?;
    if !matches!(
        credential_metadata.metadata.kind,
        SecretKind::SshPassword | SecretKind::SshPrivateKey
    ) || !global_scope(&credential_metadata)
    {
        return Err(ApiError::bad_request(
            "terminal_ssh_profile_invalid",
            "the resource SSH credential is not an active SSH password or private key",
        ));
    }
    let known_hosts_id = target.ssh_known_hosts_secret_ref.ok_or_else(|| {
        ApiError::bad_request(
            "terminal_host_key_missing",
            "the resource has no registered SSH host-key secret",
        )
    })?;
    let known_hosts_metadata = active_secret_metadata(state, known_hosts_id)?;
    if known_hosts_metadata.metadata.kind != SecretKind::SshKnownHosts
        || !global_scope(&known_hosts_metadata)
    {
        return Err(ApiError::bad_request(
            "terminal_host_key_missing",
            "the resource SSH host-key reference is invalid",
        ));
    }
    let credential = state
        .secrets
        .read(target.credential_secret_ref)
        .map_err(crate::map_secret_error)?;
    let known_hosts = state
        .secrets
        .read(known_hosts_id)
        .map_err(crate::map_secret_error)?;

    Ok(TerminalConnection {
        actor,
        target_name: target.name,
        address: target.address,
        username,
        credential_kind: credential_metadata.metadata.kind,
        credential,
        known_hosts,
    })
}

fn active_secret_metadata(
    state: &ApiState,
    secret_id: lxcup_core::SecretId,
) -> Result<StoredSecretMetadata, ApiError> {
    let metadata = state
        .secrets
        .metadata(secret_id)
        .map_err(crate::map_secret_error)?;
    if metadata.status != SecretStatus::Active {
        return Err(ApiError::bad_request(
            "terminal_ssh_profile_invalid",
            "the resource SSH secret is not active",
        ));
    }
    Ok(metadata)
}

fn global_scope(metadata: &StoredSecretMetadata) -> bool {
    metadata.metadata.scope == SecretScope::Global
}

fn valid_ssh_username(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 64
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_ssh_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'-'))
}

fn reserve_terminal_session(
    registry: Arc<TerminalSessionRegistry>,
    session_id: Uuid,
    user_id: Uuid,
    target_id: TargetId,
) -> Result<TerminalSessionLease, ApiError> {
    let mut sessions = registry.sessions.lock().map_err(|_| ApiError::storage())?;
    if sessions
        .values()
        .filter(|session| session.target_id == target_id)
        .count()
        >= MAX_SESSIONS_PER_TARGET
    {
        return Err(ApiError::conflict(
            "terminal_target_busy",
            "a terminal session is already active for this resource",
        ));
    }
    if sessions
        .values()
        .filter(|session| session.user_id == user_id)
        .count()
        >= MAX_SESSIONS_PER_USER
    {
        return Err(ApiError::conflict(
            "terminal_session_limit",
            "the administrator already has the maximum number of active terminal sessions",
        ));
    }
    sessions.insert(session_id, ActiveTerminalSession { user_id, target_id });
    drop(sessions);
    Ok(TerminalSessionLease {
        registry,
        session_id,
    })
}

async fn record_terminal_audit(
    state: &ApiState,
    actor: &AuthenticatedUser,
    target_id: TargetId,
    request_id: Option<Uuid>,
    action: &str,
    status_code: i16,
    details: serde_json::Value,
) -> Result<(), ApiError> {
    let Some(repositories) = state.repositories.as_ref() else {
        return Err(ApiError::storage());
    };
    repositories
        .auth
        .append_audit_event(&AuthAuditEvent {
            id: Uuid::new_v4(),
            actor_user_id: Some(actor.id),
            actor_username: actor.username.clone(),
            actor_role: actor.role,
            action: action.to_owned(),
            resource_type: "terminal_session".to_owned(),
            resource_id: Some(target_id.as_uuid().to_string()),
            request_id,
            status_code,
            details,
            created_at: Utc::now(),
        })
        .await
        .map_err(|_| ApiError::storage())
}

fn validate_websocket_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Err(origin_rejected());
    };
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok());
    let origin = origin
        .to_str()
        .ok()
        .and_then(|value| url::Url::parse(value).ok());
    let (Some(host), Some(origin)) = (host, origin) else {
        return Err(origin_rejected());
    };
    let host_port = host.port_u16().or_else(|| match origin.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    });
    if !matches!(origin.scheme(), "http" | "https")
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin
            .host_str()
            .is_none_or(|origin_host| !origin_host.eq_ignore_ascii_case(host.host()))
        || origin.port_or_known_default() != host_port
    {
        return Err(origin_rejected());
    }
    Ok(())
}

fn origin_rejected() -> ApiError {
    ApiError::forbidden(
        "terminal_origin_rejected",
        "the terminal request origin could not be verified",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{SecretId, SecretScope};
    use lxcup_secrets::{CreateSecret, InMemorySecretStore, SecretStore};

    #[test]
    fn terminal_session_registry_limits_sessions_per_target_and_user() {
        let registry = Arc::new(TerminalSessionRegistry::default());
        let user_id = Uuid::new_v4();
        let first_target = TargetId::new();
        let second_target = TargetId::new();
        let first_session = Uuid::new_v4();
        let first_lease =
            reserve_terminal_session(registry.clone(), first_session, user_id, first_target)
                .unwrap();

        let duplicate_target = reserve_terminal_session(
            registry.clone(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            first_target,
        )
        .err()
        .unwrap();
        assert_eq!(duplicate_target.code, "terminal_target_busy");

        let second_lease =
            reserve_terminal_session(registry.clone(), Uuid::new_v4(), user_id, second_target)
                .unwrap();
        let user_limit =
            reserve_terminal_session(registry.clone(), Uuid::new_v4(), user_id, TargetId::new())
                .err()
                .unwrap();
        assert_eq!(user_limit.code, "terminal_session_limit");

        drop(first_lease);
        let resumed =
            reserve_terminal_session(registry, first_session, user_id, first_target).unwrap();
        drop((second_lease, resumed));
    }

    #[test]
    fn terminal_websocket_origin_requires_matching_host_and_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "lxcup.example:443".parse().unwrap());
        headers.insert(header::ORIGIN, "https://lxcup.example".parse().unwrap());
        assert!(validate_websocket_origin(&headers).is_ok());

        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(validate_websocket_origin(&headers).is_err());
    }

    #[test]
    fn terminal_requires_an_admin_with_authentication_and_completed_onboarding() {
        let mut actor = AuthenticatedUser {
            id: Uuid::new_v4(),
            username: "terminal-admin".to_owned(),
            role: AuthUserRole::Admin,
            must_change_password: false,
            token_hash: "session-hash".to_owned(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        assert!(ensure_terminal_admin(true, &actor).is_ok());
        assert!(ensure_terminal_admin(false, &actor).is_err());

        actor.role = AuthUserRole::User;
        assert!(ensure_terminal_admin(true, &actor).is_err());
        actor.role = AuthUserRole::Admin;
        actor.must_change_password = true;
        assert!(ensure_terminal_admin(true, &actor).is_err());
    }

    #[tokio::test]
    async fn terminal_connection_uses_active_registered_ssh_secrets_for_managed_targets() {
        let secrets = InMemorySecretStore::default();
        let credential = secrets
            .create(CreateSecret {
                name: "terminal-test-password".to_owned(),
                kind: SecretKind::SshPassword,
                scope: SecretScope::Global,
                value: SecretValue::new("test-password").unwrap(),
            })
            .unwrap();
        let known_hosts = secrets
            .create(CreateSecret {
                name: "terminal-test-host-key".to_owned(),
                kind: SecretKind::SshKnownHosts,
                scope: SecretScope::Global,
                value: SecretValue::new("127.0.0.1 ssh-ed25519 AAAA-test-key").unwrap(),
            })
            .unwrap();
        let actor = AuthenticatedUser {
            id: Uuid::new_v4(),
            username: "terminal-admin".to_owned(),
            role: AuthUserRole::Admin,
            must_change_password: false,
            token_hash: "session-hash".to_owned(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        let state = ApiState::new().with_secret_store(Arc::new(secrets.clone()));
        let mut target = Target::new(
            "terminal-test-target",
            TargetKind::LinuxServer,
            "127.0.0.1",
            TargetTransport::Ssh,
            credential.metadata.id,
            SecretId::new(),
        )
        .unwrap();
        target.ssh_user = Some("terminal-test".to_owned());
        target.ssh_known_hosts_secret_ref = Some(known_hosts.metadata.id);
        target.mark_managed();

        let connection = load_terminal_connection(&state, actor.clone(), target.clone())
            .await
            .expect("managed SSH target should resolve its registered secrets");
        assert_eq!(connection.address, "127.0.0.1");
        assert_eq!(connection.username, "terminal-test");
        assert_eq!(connection.credential_kind, SecretKind::SshPassword);
        assert_eq!(connection.credential.expose(), "test-password");

        target.state = TargetState::Pending;
        assert!(
            load_terminal_connection(&state, actor.clone(), target.clone())
                .await
                .is_err()
        );
        target.mark_managed();
        secrets.revoke(known_hosts.metadata.id).unwrap();
        assert!(
            load_terminal_connection(&state, actor, target)
                .await
                .is_err()
        );
    }
}
