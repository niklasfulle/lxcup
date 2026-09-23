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
    pub management_state: DockerWorkloadManagementState,
    pub discovered_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerWorkloadManagementState {
    Discovered,
    Managed,
}
