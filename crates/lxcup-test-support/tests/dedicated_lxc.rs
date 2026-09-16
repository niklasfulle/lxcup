use lxcup_proxmox::{ProxmoxClient, ProxmoxClientConfig};
use lxcup_test_support::DedicatedLxcConfig;

#[tokio::test]
#[ignore = "requires an explicitly configured, dedicated integration LXC"]
async fn dedicated_lxc_is_reachable_without_using_production_credentials() {
    let config = DedicatedLxcConfig::from_env().expect("integration environment is configured");
    let client_config = ProxmoxClientConfig::new(
        &config.proxmox_base_url,
        &config.proxmox_token_id,
        &config.proxmox_token_secret,
    )
    .expect("integration Proxmox URL and credentials are valid");
    let client = ProxmoxClient::new(client_config).expect("Proxmox client can be built");
    let status = client
        .get_lxc_status(&config.node_name, config.vmid)
        .await
        .expect("dedicated integration LXC is reachable");
    assert_eq!(status.vmid, config.vmid);
}
