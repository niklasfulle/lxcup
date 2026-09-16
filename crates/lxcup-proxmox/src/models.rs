//! Typed response models for the Proxmox endpoints used by lxcup.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Summary returned by `GET /nodes`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProxmoxNodeSummary {
    pub node: String,
    pub status: Option<String>,
    pub online: Option<u8>,
    pub maxcpu: Option<u64>,
    pub maxmem: Option<u64>,
    pub mem: Option<u64>,
    pub uptime: Option<u64>,
}

/// Summary returned by `GET /nodes/{node}/lxc`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LxcContainerSummary {
    pub vmid: u64,
    pub status: String,
    pub name: Option<String>,
    pub template: Option<u8>,
    pub maxmem: Option<u64>,
    pub maxdisk: Option<u64>,
    pub disk: Option<u64>,
    pub uptime: Option<u64>,
}

/// Current status returned by `GET /nodes/{node}/lxc/{vmid}/status/current`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LxcStatus {
    pub vmid: u64,
    pub status: String,
    pub name: Option<String>,
    pub uptime: Option<u64>,
    pub cpus: Option<f64>,
    pub maxmem: Option<u64>,
    pub mem: Option<u64>,
    pub maxdisk: Option<u64>,
    pub disk: Option<u64>,
    pub template: Option<u8>,
}

/// Configuration returned by `GET /nodes/{node}/lxc/{vmid}/config`.
///
/// Proxmox can add configuration keys over time. Known keys are typed while
/// unknown keys are retained as JSON instead of breaking deserialization.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LxcConfig {
    pub hostname: Option<String>,
    pub ostype: Option<String>,
    pub cores: Option<u16>,
    pub memory: Option<u64>,
    pub swap: Option<u64>,
    pub unprivileged: Option<u8>,
    pub rootfs: Option<String>,
    pub tags: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Task state returned by `GET /nodes/{node}/tasks/{upid}/status`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProxmoxTaskStatus {
    pub upid: String,
    pub node: String,
    pub status: String,
    pub exitstatus: Option<String>,
    pub pid: Option<u64>,
    pub starttime: Option<u64>,
    #[serde(rename = "type")]
    pub task_type: Option<String>,
}

/// Request to create a named LXC snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct LxcSnapshotRequest {
    pub snapname: String,
    pub description: Option<String>,
}

/// Task identifier returned by mutating Proxmox endpoints.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProxmoxTaskStart {
    pub upid: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_fixture_accepts_optional_fields() {
        let node: ProxmoxNodeSummary =
            serde_json::from_str(r#"{"node":"pve01","status":"online","online":1,"unknown":true}"#)
                .unwrap();

        assert_eq!(node.node, "pve01");
        assert_eq!(node.online, Some(1));
        assert!(node.maxmem.is_none());
    }

    #[test]
    fn lxc_fixtures_parse_inventory_status_and_task() {
        let container: LxcContainerSummary = serde_json::from_str(
            r#"{"vmid":101,"status":"running","name":"grafana","template":0}"#,
        )
        .unwrap();
        let status: LxcStatus =
            serde_json::from_str(r#"{"vmid":101,"status":"running","uptime":42,"cpus":1}"#)
                .unwrap();
        let task: ProxmoxTaskStatus = serde_json::from_str(
            r#"{"upid":"UPID:pve01:1:2:3:vzdump:101:root@pam:","node":"pve01","status":"stopped","exitstatus":"OK","type":"vzdump"}"#,
        )
        .unwrap();

        assert_eq!(container.vmid, 101);
        assert_eq!(status.uptime, Some(42));
        assert_eq!(task.exitstatus.as_deref(), Some("OK"));
    }

    #[test]
    fn config_fixture_retains_unknown_keys() {
        let config: LxcConfig = serde_json::from_str(
            r#"{"hostname":"grafana","ostype":"debian","cores":2,"rootfs":"local-lvm:vm-101-disk-0,size=8G","feature":"nesting=1"}"#,
        )
        .unwrap();

        assert_eq!(config.hostname.as_deref(), Some("grafana"));
        assert_eq!(
            config.extra["feature"],
            Value::String("nesting=1".to_owned())
        );
    }
}
