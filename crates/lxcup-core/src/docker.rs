use crate::ContainerId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Agent-gemeldete Docker-Metadaten ohne Konfiguration, Mounts oder Secrets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerWorkload {
    pub host_container_id: ContainerId,
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<String>,
    pub started_at: Option<String>,
    pub labels: Vec<String>,
    pub presence: DockerWorkloadPresence,
    pub change: DockerWorkloadChange,
    pub management_state: DockerWorkloadManagementState,
    pub discovered_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerWorkloadChange {
    New,
    Changed,
    Unchanged,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerDiscoveryRun {
    pub id: uuid::Uuid,
    pub host_container_id: ContainerId,
    pub status: DockerDiscoveryStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub container_count: u32,
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerDiscoveryStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerWorkloadManagementState {
    Discovered,
    Managed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerWorkloadPresence {
    Present,
    Missing,
}
