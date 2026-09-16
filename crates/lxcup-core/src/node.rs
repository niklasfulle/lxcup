use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{error::DomainError, ids::NodeId};

/// Verbindungszustand eines Proxmox-Nodes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum NodeStatus {
    Unknown,
    Connected,
    Disconnected,
}

/// Ein Proxmox-Node, der von lxcup verwaltet werden kann.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub address: String,
    pub status: NodeStatus,
    pub proxmox_version: Option<String>,
    pub capabilities: Vec<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_check_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Node {
    pub fn new(name: impl Into<String>, address: impl Into<String>) -> Result<Self, DomainError> {
        let name = name.into();
        let address = address.into();

        if name.trim().is_empty() {
            return Err(DomainError::EmptyValue { field: "node name" });
        }
        if address.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "node address",
            });
        }

        Ok(Self {
            id: NodeId::new(),
            name,
            address,
            status: NodeStatus::Unknown,
            proxmox_version: None,
            capabilities: Vec::new(),
            last_checked_at: None,
            last_check_error: None,
            created_at: Utc::now(),
        })
    }

    /// Stores a successful Proxmox capability check on the node.
    pub fn record_check_success(
        &mut self,
        version: impl Into<String>,
        capabilities: Vec<String>,
        checked_at: DateTime<Utc>,
    ) {
        self.status = NodeStatus::Connected;
        self.proxmox_version = Some(version.into());
        self.capabilities = capabilities;
        self.last_checked_at = Some(checked_at);
        self.last_check_error = None;
    }

    /// Stores a failed Proxmox capability check without discarding identity data.
    pub fn record_check_failure(&mut self, error: impl Into<String>, checked_at: DateTime<Utc>) {
        self.status = NodeStatus::Disconnected;
        self.last_checked_at = Some(checked_at);
        self.last_check_error = Some(error.into());
    }
}

#[cfg(test)]
mod tests {
    use super::{Node, NodeStatus};

    #[test]
    fn creates_unknown_node() {
        let node = Node::new("pve01", "https://pve01:8006").unwrap();

        assert_eq!(node.name, "pve01");
        assert_eq!(node.status, NodeStatus::Unknown);
        assert!(node.proxmox_version.is_none());
    }

    #[test]
    fn rejects_empty_name() {
        assert!(Node::new(" ", "https://pve01:8006").is_err());
    }

    #[test]
    fn records_successful_and_failed_checks() {
        let checked_at = chrono::DateTime::from_timestamp(1_704_067_200, 0).unwrap();
        let mut node = Node::new("pve01", "https://pve01:8006").unwrap();

        node.record_check_success(
            "8.4.1",
            vec!["version.info".to_owned(), "node.info".to_owned()],
            checked_at,
        );
        assert_eq!(node.status, NodeStatus::Connected);
        assert_eq!(node.proxmox_version.as_deref(), Some("8.4.1"));
        assert!(node.last_check_error.is_none());

        node.record_check_failure("permission denied", checked_at);
        assert_eq!(node.status, NodeStatus::Disconnected);
        assert_eq!(node.last_check_error.as_deref(), Some("permission denied"));
        assert_eq!(node.proxmox_version.as_deref(), Some("8.4.1"));
    }
}
