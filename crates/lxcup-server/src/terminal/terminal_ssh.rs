use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};

use axum::extract::ws::{Message, WebSocket};
use chrono::Utc;
use futures_util::StreamExt;
use lxcup_core::SecretKind;
use russh::{ChannelMsg, client, keys::PrivateKeyWithHashAlg};
use serde::Deserialize;
use tokio::time::{Instant, sleep_until, timeout};

use super::TerminalConnection;
use crate::{ApiState, AuthenticatedUser};

const SSH_PORT: u16 = 22;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);
const AUTH_RECHECK_INTERVAL: Duration = Duration::from_secs(15);
const MAX_INPUT_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Input { data: String },
    Resize { columns: u32, rows: u32 },
    Close,
}

struct KnownHostsHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for KnownHostsHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(russh::keys::known_hosts::check_known_hosts_path(
            &self.host,
            self.port,
            server_public_key,
            &self.known_hosts,
        )
        .unwrap_or(false))
    }
}

pub(super) async fn run_session(
    mut socket: WebSocket,
    state: ApiState,
    connection: TerminalConnection,
) -> Result<(), &'static str> {
    let actor = connection.actor.clone();
    let (handle, mut channel, _known_hosts_file) = match open_ssh_session(&connection).await {
        Ok(session) => session,
        Err(reason) => {
            send_error(
                &mut socket,
                "SSH-Verbindung oder Host-Key-Prüfung fehlgeschlagen.",
            )
            .await;
            return Err(reason);
        }
    };
    if let Err(error) = send_json(&mut socket, serde_json::json!({"type":"ready"})).await {
        let _ = channel.close().await;
        let _ = handle
            .disconnect(
                russh::Disconnect::ByApplication,
                "terminal client disconnected during setup",
                "en",
            )
            .await;
        return Err(error);
    }

    let started = Instant::now();
    let maximum_deadline = started + MAX_SESSION_DURATION;
    let mut idle_deadline = started + IDLE_TIMEOUT;
    let mut auth_check = tokio::time::interval(AUTH_RECHECK_INTERVAL);
    auth_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result = loop {
        tokio::select! {
            _ = sleep_until(maximum_deadline) => {
                if send_json(&mut socket, serde_json::json!({"type":"closed","reason":"maximum_duration"})).await.is_err() {
                    break Err("websocket_send_failed");
                }
                break Err("maximum_duration");
            }
            _ = sleep_until(idle_deadline) => {
                if send_json(&mut socket, serde_json::json!({"type":"closed","reason":"idle_timeout"})).await.is_err() {
                    break Err("websocket_send_failed");
                }
                break Err("idle_timeout");
            }
            _ = auth_check.tick() => {
                if !session_is_admin(&state, &actor).await {
                    send_error(&mut socket, "Deine Administratorsitzung ist nicht mehr gültig.").await;
                    break Err("session_expired");
                }
            }
            incoming = socket.next() => {
                if !session_is_admin(&state, &actor).await {
                    send_error(&mut socket, "Deine Administratorsitzung ist nicht mehr gültig.").await;
                    break Err("session_expired");
                }
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(text.as_str()) {
                            Ok(ClientMessage::Input { data }) if data.len() <= MAX_INPUT_BYTES => {
                                idle_deadline = Instant::now() + IDLE_TIMEOUT;
                                if channel.data_bytes(data.into_bytes()).await.is_err() {
                                    break Err("ssh_stream_failed");
                                }
                            }
                            Ok(ClientMessage::Resize { columns, rows })
                                if valid_terminal_dimensions(columns, rows) =>
                            {
                                idle_deadline = Instant::now() + IDLE_TIMEOUT;
                                if channel.window_change(columns, rows, 0, 0).await.is_err() {
                                    break Err("ssh_stream_failed");
                                }
                            }
                            Ok(ClientMessage::Close) => break Ok(()),
                            _ => {
                                send_error(&mut socket, "Ungültige oder zu große Terminalnachricht.").await;
                                break Err("invalid_client_message");
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) if data.len() <= MAX_INPUT_BYTES => {
                        idle_deadline = Instant::now() + IDLE_TIMEOUT;
                        if channel.data_bytes(data).await.is_err() {
                            break Err("ssh_stream_failed");
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break Ok(()),
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break Ok(());
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {
                        send_error(&mut socket, "Die Terminalnachricht ist zu groß.").await;
                        break Err("oversized_client_message");
                    }
                    Some(Err(_)) => break Err("websocket_failed"),
                }
            }
            incoming = channel.wait() => {
                let Some(message) = incoming else {
                    break Ok(());
                };
                match message {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                        if !session_is_admin(&state, &actor).await {
                            send_error(&mut socket, "Deine Administratorsitzung ist nicht mehr gültig.").await;
                            break Err("session_expired");
                        }
                        idle_deadline = Instant::now() + IDLE_TIMEOUT;
                        if socket.send(Message::Binary(data)).await.is_err() {
                            break Ok(());
                        }
                    }
                    ChannelMsg::ExitStatus { .. } | ChannelMsg::Close => break Ok(()),
                    _ => {}
                }
            }
        }
    };

    let _ = channel.close().await;
    let _ = handle
        .disconnect(
            russh::Disconnect::ByApplication,
            "terminal session ended",
            "en",
        )
        .await;
    result
}

fn valid_terminal_dimensions(columns: u32, rows: u32) -> bool {
    (5..=300).contains(&columns) && (2..=120).contains(&rows)
}

async fn open_ssh_session(
    connection: &TerminalConnection,
) -> Result<
    (
        client::Handle<KnownHostsHandler>,
        russh::Channel<russh::client::Msg>,
        tempfile::TempPath,
    ),
    &'static str,
> {
    open_ssh_session_at(connection, SSH_PORT).await
}

async fn open_ssh_session_at(
    connection: &TerminalConnection,
    port: u16,
) -> Result<
    (
        client::Handle<KnownHostsHandler>,
        russh::Channel<russh::client::Msg>,
        tempfile::TempPath,
    ),
    &'static str,
> {
    let mut hosts_file = tempfile::NamedTempFile::new().map_err(|_| "known_hosts_unavailable")?;
    hosts_file
        .write_all(connection.known_hosts.expose().as_bytes())
        .map_err(|_| "known_hosts_unavailable")?;
    let known_hosts_file = hosts_file.into_temp_path();
    let config = client::Config {
        inactivity_timeout: Some(IDLE_TIMEOUT),
        keepalive_interval: Some(Duration::from_secs(20)),
        ..client::Config::default()
    };
    let handler = KnownHostsHandler {
        host: connection.address.clone(),
        port,
        known_hosts: known_hosts_file.to_path_buf(),
    };
    let mut handle = timeout(
        CONNECT_TIMEOUT,
        client::connect(
            Arc::new(config),
            (connection.address.as_str(), port),
            handler,
        ),
    )
    .await
    .map_err(|_| "ssh_connect_timeout")?
    .map_err(|_| "ssh_connect_failed")?;

    let authenticated = match connection.credential_kind {
        SecretKind::SshPassword => timeout(
            CONNECT_TIMEOUT,
            handle.authenticate_password(
                connection.username.clone(),
                connection.credential.expose().to_owned(),
            ),
        )
        .await
        .map_err(|_| "ssh_authentication_timeout")?
        .map_err(|_| "ssh_authentication_failed")?
        .success(),
        SecretKind::SshPrivateKey => {
            let private_key = russh::keys::decode_secret_key(connection.credential.expose(), None)
                .map_err(|_| "ssh_private_key_invalid")?;
            let hash_algorithm = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|_| "ssh_authentication_failed")?
                .flatten();
            timeout(
                CONNECT_TIMEOUT,
                handle.authenticate_publickey(
                    connection.username.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_algorithm),
                ),
            )
            .await
            .map_err(|_| "ssh_authentication_timeout")?
            .map_err(|_| "ssh_authentication_failed")?
            .success()
        }
        _ => return Err("ssh_credential_invalid"),
    };
    if !authenticated {
        return Err("ssh_authentication_failed");
    }

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|_| "ssh_session_failed")?;
    channel
        .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .map_err(|_| "ssh_pty_failed")?;
    wait_for_request_success(&mut channel, "ssh_pty_failed").await?;
    channel
        .request_shell(true)
        .await
        .map_err(|_| "ssh_shell_failed")?;
    wait_for_request_success(&mut channel, "ssh_shell_failed").await?;
    Ok((handle, channel, known_hosts_file))
}

async fn wait_for_request_success(
    channel: &mut russh::Channel<russh::client::Msg>,
    failure: &'static str,
) -> Result<(), &'static str> {
    match channel.wait().await {
        Some(ChannelMsg::Success) => Ok(()),
        _ => Err(failure),
    }
}

async fn session_is_admin(state: &ApiState, actor: &AuthenticatedUser) -> bool {
    let Some(repositories) = state.repositories.as_ref() else {
        return false;
    };
    let Ok(Some(session)) = repositories
        .auth
        .session_by_token_hash(&actor.token_hash)
        .await
    else {
        return false;
    };
    session.user.id == actor.id
        && session.user.role == lxcup_persistence::AuthUserRole::Admin
        && !session.user.disabled
        && !session.user.must_change_password
        && session.expires_at > Utc::now()
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), &'static str> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| "websocket_send_failed")
}

async fn send_error(socket: &mut WebSocket, message: &str) {
    let _ = send_json(
        socket,
        serde_json::json!({"type":"error", "message":message}),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use russh::{
        Channel, ChannelId, Error,
        keys::Algorithm,
        server::{self, Msg, Server as _, Session},
    };
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;
    use crate::AuthenticatedUser;

    #[test]
    fn terminal_dimensions_allow_small_screens_but_remain_bounded() {
        assert!(valid_terminal_dimensions(5, 2));
        assert!(valid_terminal_dimensions(300, 120));
        assert!(!valid_terminal_dimensions(4, 24));
        assert!(!valid_terminal_dimensions(301, 24));
        assert!(!valid_terminal_dimensions(80, 121));
    }

    #[derive(Clone)]
    struct PtyTestServer {
        pty_requested: Arc<AtomicBool>,
        shell_requested: Arc<AtomicBool>,
        reject_pty: Arc<AtomicBool>,
        reject_shell: Arc<AtomicBool>,
    }

    impl server::Server for PtyTestServer {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for PtyTestServer {
        type Error = Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> Result<server::Auth, Self::Error> {
            Ok(if user == "terminal-test" && password == "test-password" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            term: &str,
            _: u32,
            _: u32,
            _: u32,
            _: u32,
            _: &[(russh::Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.pty_requested.store(!term.is_empty(), Ordering::SeqCst);
            if self.reject_pty.load(Ordering::SeqCst) {
                session.channel_failure(channel)?;
            } else {
                session.channel_success(channel)?;
            }
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.shell_requested.store(true, Ordering::SeqCst);
            if self.reject_shell.load(Ordering::SeqCst) {
                session.channel_failure(channel)?;
            } else {
                session.channel_success(channel)?;
            }
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.data(channel, data.to_vec())?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn ssh_session_pins_host_key_opens_pty_shell_and_round_trips_data() {
        let host_key = russh::keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .expect("generate isolated SSH host key");
        let host_public_key = host_key
            .public_key()
            .to_openssh()
            .expect("encode isolated SSH host key");
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..server::Config::default()
        });
        let pty_requested = Arc::new(AtomicBool::new(false));
        let shell_requested = Arc::new(AtomicBool::new(false));
        let reject_pty = Arc::new(AtomicBool::new(false));
        let reject_shell = Arc::new(AtomicBool::new(false));
        let server_impl = PtyTestServer {
            pty_requested: pty_requested.clone(),
            shell_requested: shell_requested.clone(),
            reject_pty: reject_pty.clone(),
            reject_shell: reject_shell.clone(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let mut server_impl = server_impl;
            let server = server_impl.run_on_socket(config, &listener);
            let shutdown = server.handle();
            let _ = shutdown_sender.send(shutdown);
            server.await
        });
        let shutdown = shutdown_receiver.await.unwrap();

        let actor = AuthenticatedUser {
            id: Uuid::new_v4(),
            username: "terminal-admin".to_owned(),
            role: lxcup_persistence::AuthUserRole::Admin,
            must_change_password: false,
            token_hash: "test-session-token-hash".to_owned(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        let connection = TerminalConnection {
            actor,
            target_name: "isolated-test-target".to_owned(),
            address: "127.0.0.1".to_owned(),
            username: "terminal-test".to_owned(),
            credential_kind: SecretKind::SshPassword,
            credential: lxcup_core::SecretValue::new("test-password").unwrap(),
            known_hosts: lxcup_core::SecretValue::new(format!(
                "[127.0.0.1]:{} {host_public_key}",
                address.port()
            ))
            .unwrap(),
        };

        let (handle, mut channel, _known_hosts) = open_ssh_session_at(&connection, address.port())
            .await
            .expect("connect, verify pinned key, and open PTY shell");
        assert!(pty_requested.load(Ordering::SeqCst));
        assert!(shell_requested.load(Ordering::SeqCst));
        channel
            .data_bytes(b"terminal round trip\n".to_vec())
            .await
            .unwrap();
        let response = timeout(Duration::from_secs(2), channel.wait())
            .await
            .expect("SSH test server should echo input")
            .expect("SSH echo channel remains open");
        let ChannelMsg::Data { data } = response else {
            panic!("expected echoed SSH PTY data");
        };
        assert_eq!(data.as_ref(), b"terminal round trip\n");

        let _ = channel.close().await;
        let _ = handle
            .disconnect(russh::Disconnect::ByApplication, "test complete", "en")
            .await;

        let pinned_hosts = lxcup_core::SecretValue::new(format!(
            "[127.0.0.1]:{} {host_public_key}",
            address.port()
        ))
        .unwrap();
        let mut rejected_password = TerminalConnection {
            actor: AuthenticatedUser {
                id: Uuid::new_v4(),
                username: "terminal-admin".to_owned(),
                role: lxcup_persistence::AuthUserRole::Admin,
                must_change_password: false,
                token_hash: "test-session-token-hash".to_owned(),
                expires_at: Utc::now() + chrono::Duration::hours(1),
            },
            target_name: "isolated-test-target".to_owned(),
            address: "127.0.0.1".to_owned(),
            username: "terminal-test".to_owned(),
            credential_kind: SecretKind::SshPassword,
            credential: lxcup_core::SecretValue::new("wrong-password").unwrap(),
            known_hosts: pinned_hosts.clone(),
        };
        assert!(matches!(
            open_ssh_session_at(&rejected_password, address.port()).await,
            Err("ssh_authentication_failed")
        ));

        rejected_password.credential_kind = SecretKind::SshKnownHosts;
        assert!(matches!(
            open_ssh_session_at(&rejected_password, address.port()).await,
            Err("ssh_credential_invalid")
        ));

        rejected_password.credential_kind = SecretKind::SshPrivateKey;
        rejected_password.credential = lxcup_core::SecretValue::new("not a private key").unwrap();
        assert!(matches!(
            open_ssh_session_at(&rejected_password, address.port()).await,
            Err("ssh_private_key_invalid")
        ));

        rejected_password.credential_kind = SecretKind::SshPassword;
        rejected_password.credential = lxcup_core::SecretValue::new("test-password").unwrap();
        reject_pty.store(true, Ordering::SeqCst);
        assert!(matches!(
            open_ssh_session_at(&rejected_password, address.port()).await,
            Err("ssh_pty_failed")
        ));
        reject_pty.store(false, Ordering::SeqCst);
        reject_shell.store(true, Ordering::SeqCst);
        assert!(matches!(
            open_ssh_session_at(&rejected_password, address.port()).await,
            Err("ssh_shell_failed")
        ));

        let wrong_host_key = russh::keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .expect("generate mismatched SSH host key");
        let wrong_public_key = wrong_host_key
            .public_key()
            .to_openssh()
            .expect("encode mismatched SSH host key");
        let mut wrong_pin_connection = connection;
        wrong_pin_connection.known_hosts = lxcup_core::SecretValue::new(format!(
            "[127.0.0.1]:{} {wrong_public_key}",
            address.port()
        ))
        .unwrap();
        assert!(matches!(
            open_ssh_session_at(&wrong_pin_connection, address.port()).await,
            Err("ssh_connect_failed")
        ));

        shutdown.shutdown("test complete".into());
        server_task.await.unwrap().unwrap();
    }
}
