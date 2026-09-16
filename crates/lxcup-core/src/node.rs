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
            created_at: Utc::now(),
        })
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
    }

    #[test]
    fn rejects_empty_name() {
        assert!(Node::new(" ", "https://pve01:8006").is_err());
    }
}
