//! Controlled polling for long-running Proxmox tasks.

use std::{future::Future, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{Instant, sleep, timeout};

use crate::{ProxmoxClient, ProxmoxClientError, models::ProxmoxTaskStatus};

/// Polling limits for one Proxmox task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskPollingConfig {
    pub interval: Duration,
    pub timeout: Duration,
}

impl TaskPollingConfig {
    pub fn new(interval: Duration, timeout: Duration) -> Result<Self, TaskPollingError> {
        if interval.is_zero() || timeout.is_zero() || interval > timeout {
            return Err(TaskPollingError::InvalidConfig);
        }
        Ok(Self { interval, timeout })
    }
}

/// State classification used by backend and future SSE consumers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TaskState {
    Running,
    Succeeded,
    Failed,
    Unknown,
}

/// A state transition observed once during polling.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskEvent {
    pub upid: String,
    pub state: TaskState,
    pub status: String,
    pub exit_status: Option<String>,
    pub observed_at: DateTime<Utc>,
}

/// Terminal result of a polling run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TaskOutcome {
    Succeeded,
    Failed { exit_status: Option<String> },
    TimedOut,
}

/// Polling result with duplicate state observations removed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskPollResult {
    pub outcome: TaskOutcome,
    pub events: Vec<TaskEvent>,
}

/// Errors that prevent a task from being polled.
#[derive(Debug, Error)]
pub enum TaskPollingError {
    #[error("task polling interval and timeout must be non-zero, with interval <= timeout")]
    InvalidConfig,
    #[error("task polling request timed out")]
    Timeout,
    #[error("task polling request failed: {0}")]
    Transport(#[source] ProxmoxClientError),
}

/// Polls a Proxmox task until it finishes or the configured timeout expires.
pub struct TaskPoller {
    config: TaskPollingConfig,
}

impl TaskPoller {
    pub fn new(config: TaskPollingConfig) -> Self {
        Self { config }
    }

    pub async fn poll_task(
        &self,
        client: &ProxmoxClient,
        node_name: &str,
        upid: &str,
    ) -> Result<TaskPollResult, TaskPollingError> {
        self.poll_with(|| client.get_task_status(node_name, upid))
            .await
    }

    async fn poll_with<F, Fut>(
        &self,
        mut read_status: F,
    ) -> Result<TaskPollResult, TaskPollingError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<ProxmoxTaskStatus, ProxmoxClientError>>,
    {
        let deadline = Instant::now() + self.config.timeout;
        let mut events = Vec::new();
        let mut last_fingerprint: Option<TaskFingerprint> = None;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(TaskPollResult {
                    outcome: TaskOutcome::TimedOut,
                    events,
                });
            }

            let status = timeout(remaining, read_status())
                .await
                .map_err(|_| TaskPollingError::Timeout)?
                .map_err(TaskPollingError::Transport)?;
            let state = classify(&status);
            let fingerprint = TaskFingerprint::from_status(&status, &state);

            if last_fingerprint.as_ref() != Some(&fingerprint) {
                events.push(TaskEvent {
                    upid: status.upid.clone(),
                    state: state.clone(),
                    status: status.status.clone(),
                    exit_status: status.exitstatus.clone(),
                    observed_at: Utc::now(),
                });
                last_fingerprint = Some(fingerprint);
            }

            match state {
                TaskState::Succeeded => {
                    return Ok(TaskPollResult {
                        outcome: TaskOutcome::Succeeded,
                        events,
                    });
                }
                TaskState::Failed => {
                    return Ok(TaskPollResult {
                        outcome: TaskOutcome::Failed {
                            exit_status: status.exitstatus,
                        },
                        events,
                    });
                }
                TaskState::Running | TaskState::Unknown => sleep(self.config.interval).await,
            }
        }
    }
}

impl From<ProxmoxClientError> for TaskPollingError {
    fn from(error: ProxmoxClientError) -> Self {
        Self::Transport(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TaskFingerprint {
    upid: String,
    state: TaskState,
    status: String,
    exit_status: Option<String>,
}

impl TaskFingerprint {
    fn from_status(status: &ProxmoxTaskStatus, state: &TaskState) -> Self {
        Self {
            upid: status.upid.clone(),
            state: state.clone(),
            status: status.status.clone(),
            exit_status: status.exitstatus.clone(),
        }
    }
}

fn classify(status: &ProxmoxTaskStatus) -> TaskState {
    match (status.status.as_str(), status.exitstatus.as_deref()) {
        ("running", _) => TaskState::Running,
        ("stopped", Some("OK")) => TaskState::Succeeded,
        ("stopped", _) => TaskState::Failed,
        _ => TaskState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;

    fn status(status: &str, exitstatus: Option<&str>) -> ProxmoxTaskStatus {
        ProxmoxTaskStatus {
            upid: "UPID:pve01:1:2:3:vzdump:101:root@pam:".to_owned(),
            node: "pve01".to_owned(),
            status: status.to_owned(),
            exitstatus: exitstatus.map(str::to_owned),
            pid: None,
            starttime: None,
            task_type: Some("vzdump".to_owned()),
        }
    }

    #[test]
    fn rejects_unbounded_polling_configuration() {
        assert!(TaskPollingConfig::new(Duration::ZERO, Duration::from_secs(1)).is_err());
        assert!(TaskPollingConfig::new(Duration::from_secs(2), Duration::from_secs(1)).is_err());
    }

    #[tokio::test]
    async fn deduplicates_running_states_and_returns_success() {
        let config =
            TaskPollingConfig::new(Duration::from_millis(1), Duration::from_secs(1)).unwrap();
        let poller = TaskPoller::new(config);
        let statuses = Arc::new(Mutex::new(VecDeque::from([
            status("running", None),
            status("running", None),
            status("stopped", Some("OK")),
        ])));

        let result =
            poller
                .poll_with(|| {
                    let statuses = Arc::clone(&statuses);
                    async move {
                        Ok::<_, ProxmoxClientError>(statuses.lock().unwrap().pop_front().unwrap())
                    }
                })
                .await
                .unwrap();

        assert_eq!(result.outcome, TaskOutcome::Succeeded);
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].state, TaskState::Running);
        assert_eq!(result.events[1].state, TaskState::Succeeded);
    }

    #[tokio::test]
    async fn timeout_returns_controlled_outcome() {
        let config =
            TaskPollingConfig::new(Duration::from_millis(2), Duration::from_millis(8)).unwrap();
        let poller = TaskPoller::new(config);

        let result = poller
            .poll_with(|| async { Ok::<_, ProxmoxClientError>(status("running", None)) })
            .await
            .unwrap();

        assert_eq!(result.outcome, TaskOutcome::TimedOut);
        assert_eq!(result.events.len(), 1);
    }
}
