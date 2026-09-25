//! Optional preflight and postflight safety checks for update executions.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{net::TcpStream, time::timeout};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum SnapshotState {
    NotRequested,
    Succeeded { task_id: String },
    Failed { reason: String },
}

pub fn evaluate_snapshot(requested: bool, result: Result<String, String>) -> SnapshotState {
    if !requested {
        return SnapshotState::NotRequested;
    }
    match result {
        Ok(task_id) if !task_id.trim().is_empty() => SnapshotState::Succeeded { task_id },
        Ok(_) => SnapshotState::Failed {
            reason: "snapshot task id was empty".to_owned(),
        },
        Err(reason) => SnapshotState::Failed { reason },
    }
}

pub fn snapshot_allows_execution(state: &SnapshotState) -> bool {
    matches!(
        state,
        SnapshotState::NotRequested | SnapshotState::Succeeded { .. }
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthCheckSpec {
    Http { url: String },
    Tcp { host: String, port: u16 },
    Systemd { unit: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HealthCheckResult {
    pub check: HealthCheckSpec,
    pub healthy: bool,
    pub detail: String,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum RebootRequirement {
    Required,
    NotRequired,
    Unknown,
}

pub fn parse_reboot_requirement(stdout: &str, exit_code: Option<i32>) -> RebootRequirement {
    if exit_code != Some(0) {
        return RebootRequirement::Unknown;
    }
    match stdout.trim().to_ascii_lowercase().as_str() {
        "required" | "yes" | "true" | "1" => RebootRequirement::Required,
        "not_required" | "no" | "false" | "0" => RebootRequirement::NotRequired,
        _ => RebootRequirement::Unknown,
    }
}

#[derive(Clone, Debug)]
pub struct HealthChecker {
    client: reqwest::Client,
    timeout: Duration,
}

impl HealthChecker {
    pub fn new(timeout: Duration) -> Result<Self, SafetyError> {
        if timeout.is_zero() {
            return Err(SafetyError::InvalidTimeout);
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| SafetyError::ClientBuild)?;
        Ok(Self { client, timeout })
    }

    pub async fn run(&self, check: HealthCheckSpec) -> HealthCheckResult {
        let (healthy, detail) = match &check {
            HealthCheckSpec::Http { url } => match self.client.get(url).send().await {
                Ok(response) => (
                    response.status().is_success(),
                    format!("HTTP {}", response.status().as_u16()),
                ),
                Err(error) => (false, safe_error_detail(&error.to_string())),
            },
            HealthCheckSpec::Tcp { host, port } => {
                match timeout(self.timeout, TcpStream::connect((host.as_str(), *port))).await {
                    Ok(Ok(_)) => (true, "TCP connection established".to_owned()),
                    Ok(Err(error)) => (false, safe_error_detail(&error.to_string())),
                    Err(_) => (false, "TCP healthcheck timed out".to_owned()),
                }
            }
            HealthCheckSpec::Systemd { unit } => {
                (false, format!("systemd probe requires an agent: {unit}"))
            }
        };
        HealthCheckResult {
            check,
            healthy,
            detail,
            checked_at: chrono::Utc::now(),
        }
    }
}

fn safe_error_detail(detail: &str) -> String {
    detail
        .replace(['\r', '\n'], " ")
        .chars()
        .take(240)
        .collect()
}

#[derive(Debug, Error)]
pub enum SafetyError {
    #[error("healthcheck timeout must be greater than zero")]
    InvalidTimeout,
    #[error("healthcheck client could not be created")]
    ClientBuild,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_requested_snapshot_fails_closed() {
        let failed = evaluate_snapshot(true, Err("timeout".to_owned()));
        assert!(!snapshot_allows_execution(&failed));
        assert!(snapshot_allows_execution(&evaluate_snapshot(
            false,
            Err("ignored".to_owned())
        )));
    }

    #[test]
    fn reboot_output_has_an_unknown_fallback() {
        assert_eq!(
            parse_reboot_requirement("required", Some(0)),
            RebootRequirement::Required
        );
        assert_eq!(
            parse_reboot_requirement("not_required", Some(0)),
            RebootRequirement::NotRequired
        );
        assert_eq!(
            parse_reboot_requirement("wat", Some(0)),
            RebootRequirement::Unknown
        );
        assert_eq!(
            parse_reboot_requirement("required", Some(1)),
            RebootRequirement::Unknown
        );
    }

    #[tokio::test]
    async fn systemd_checks_are_explicitly_agent_bound() {
        let checker = HealthChecker::new(Duration::from_millis(50)).unwrap();
        let result = checker
            .run(HealthCheckSpec::Systemd {
                unit: "ssh.service".to_owned(),
            })
            .await;
        assert!(!result.healthy);
        assert!(result.detail.contains("agent"));
    }

    #[test]
    fn snapshots_require_a_nonblank_successful_task_id() {
        assert_eq!(
            evaluate_snapshot(false, Ok("ignored".to_owned())),
            SnapshotState::NotRequested
        );
        assert_eq!(
            evaluate_snapshot(true, Ok(" task-17 ".to_owned())),
            SnapshotState::Succeeded {
                task_id: " task-17 ".to_owned()
            }
        );
        let empty = evaluate_snapshot(true, Ok("  ".to_owned()));
        assert_eq!(
            empty,
            SnapshotState::Failed {
                reason: "snapshot task id was empty".to_owned()
            }
        );
        assert!(!snapshot_allows_execution(&empty));
        assert!(snapshot_allows_execution(&evaluate_snapshot(
            true,
            Ok("task-18".to_owned())
        )));
    }

    #[test]
    fn reboot_requirement_accepts_case_and_numeric_markers() {
        for value in ["YES", "True", "1"] {
            assert_eq!(
                parse_reboot_requirement(value, Some(0)),
                RebootRequirement::Required
            );
        }
        for value in ["NO", "False", "0"] {
            assert_eq!(
                parse_reboot_requirement(value, Some(0)),
                RebootRequirement::NotRequired
            );
        }
    }

    #[tokio::test]
    async fn tcp_checker_reports_success_and_connection_failure() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let checker = HealthChecker::new(Duration::from_secs(1)).unwrap();
        let success = checker
            .run(HealthCheckSpec::Tcp {
                host: "127.0.0.1".to_owned(),
                port,
            })
            .await;
        assert!(success.healthy);

        drop(listener);
        let failure = checker
            .run(HealthCheckSpec::Tcp {
                host: "127.0.0.1".to_owned(),
                port,
            })
            .await;
        assert!(!failure.healthy);
        assert!(!failure.detail.is_empty());
    }

    #[tokio::test]
    async fn invalid_http_endpoint_and_zero_timeout_fail_safely() {
        assert!(matches!(
            HealthChecker::new(Duration::ZERO),
            Err(SafetyError::InvalidTimeout)
        ));
        let checker = HealthChecker::new(Duration::from_millis(50)).unwrap();
        let result = checker
            .run(HealthCheckSpec::Http {
                url: "not a url".to_owned(),
            })
            .await;
        assert!(!result.healthy);
        assert!(result.detail.len() <= 240);
    }
}
