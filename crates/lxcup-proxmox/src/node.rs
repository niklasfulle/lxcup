//! Proxmox node registration and capability checks.

use serde::Deserialize;
use thiserror::Error;

use crate::{ProxmoxClient, ProxmoxClientError};

/// Result of checking a Proxmox node identity and basic API capabilities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxmoxNodeCheck {
    pub node_name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

/// Checks the API identity and capabilities of one Proxmox node.
pub async fn check_node(
    client: &ProxmoxClient,
    node_name: &str,
) -> Result<ProxmoxNodeCheck, NodeCheckError> {
    validate_node_name(node_name)?;

    let version: VersionResponse = client.get_json("/version").await?;
    let node: NodeResponse = client.get_json(&format!("/nodes/{node_name}")).await?;
    if node.node != node_name {
        return Err(NodeCheckError::UnexpectedIdentity {
            expected: node_name.to_owned(),
            actual: node.node,
        });
    }

    Ok(ProxmoxNodeCheck {
        node_name: node_name.to_owned(),
        version: version.version,
        capabilities: vec!["version.info".to_owned(), "node.info".to_owned()],
    })
}

fn validate_node_name(node_name: &str) -> Result<(), NodeCheckError> {
    if node_name.is_empty()
        || !node_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err(NodeCheckError::InvalidNodeName);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
    version: String,
}

#[derive(Debug, Deserialize)]
struct NodeResponse {
    node: String,
}

/// Errors returned while checking a Proxmox node.
#[derive(Debug, Error)]
pub enum NodeCheckError {
    #[error("the Proxmox node name is invalid")]
    InvalidNodeName,
    #[error("the Proxmox node check failed: {0}")]
    Transport(#[source] ProxmoxClientError),
    #[error("the Proxmox API identified node {actual:?}, expected {expected:?}")]
    UnexpectedIdentity { expected: String, actual: String },
}

impl From<ProxmoxClientError> for NodeCheckError {
    fn from(error: ProxmoxClientError) -> Self {
        Self::Transport(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeCheckError, validate_node_name};

    #[test]
    fn node_names_are_single_safe_path_segments() {
        assert!(validate_node_name("pve01").is_ok());
        assert!(validate_node_name("pve-prod_01").is_ok());
        assert!(matches!(
            validate_node_name("../etc"),
            Err(NodeCheckError::InvalidNodeName)
        ));
        assert!(matches!(
            validate_node_name(""),
            Err(NodeCheckError::InvalidNodeName)
        ));
    }
}
