//! Typed accessors for Proxmox inventory, LXC and task endpoints.

use crate::{ProxmoxClient, ProxmoxClientError, models::*, node::validate_node_name};

impl ProxmoxClient {
    pub async fn list_nodes(&self) -> Result<Vec<ProxmoxNodeSummary>, ProxmoxClientError> {
        self.get_json("/nodes").await
    }

    pub async fn list_lxc(
        &self,
        node_name: &str,
    ) -> Result<Vec<LxcContainerSummary>, ProxmoxClientError> {
        validate_node_name(node_name).map_err(|_| ProxmoxClientError::InvalidPath)?;
        self.get_json(&format!("/nodes/{node_name}/lxc")).await
    }

    pub async fn get_lxc_status(
        &self,
        node_name: &str,
        vmid: u64,
    ) -> Result<LxcStatus, ProxmoxClientError> {
        validate_lxc_path(node_name, vmid)?;
        self.get_json(&format!("/nodes/{node_name}/lxc/{vmid}/status/current"))
            .await
    }

    pub async fn get_lxc_config(
        &self,
        node_name: &str,
        vmid: u64,
    ) -> Result<LxcConfig, ProxmoxClientError> {
        validate_lxc_path(node_name, vmid)?;
        self.get_json(&format!("/nodes/{node_name}/lxc/{vmid}/config"))
            .await
    }

    pub async fn get_task_status(
        &self,
        node_name: &str,
        upid: &str,
    ) -> Result<ProxmoxTaskStatus, ProxmoxClientError> {
        validate_node_name(node_name).map_err(|_| ProxmoxClientError::InvalidPath)?;
        if upid.is_empty()
            || !upid
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ":-_".contains(character))
        {
            return Err(ProxmoxClientError::InvalidPath);
        }
        self.get_json(&format!("/nodes/{node_name}/tasks/{upid}/status"))
            .await
    }
}

fn validate_lxc_path(node_name: &str, vmid: u64) -> Result<(), ProxmoxClientError> {
    validate_node_name(node_name).map_err(|_| ProxmoxClientError::InvalidPath)?;
    if vmid == 0 {
        return Err(ProxmoxClientError::InvalidPath);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_endpoint_segments_are_rejected_before_http() {
        assert!(validate_lxc_path("pve01", 101).is_ok());
        assert!(validate_lxc_path("../pve", 101).is_err());
        assert!(validate_lxc_path("pve01", 0).is_err());
    }
}
