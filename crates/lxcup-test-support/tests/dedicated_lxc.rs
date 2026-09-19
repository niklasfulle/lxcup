use std::net::{IpAddr, SocketAddr};

use lxcup_proxmox::{ProxmoxClient, ProxmoxClientConfig};
use lxcup_test_support::DedicatedLxcConfig;
use url::Url;

#[tokio::test]
#[ignore = "requires an explicitly configured, dedicated integration LXC"]
async fn dedicated_lxc_is_reachable_without_using_production_credentials() {
    let config = DedicatedLxcConfig::from_env().expect("integration environment is configured");
    let mut client_config = ProxmoxClientConfig::new(
        &config.proxmox_base_url,
        &config.proxmox_token_id,
        &config.proxmox_token_secret,
    )
    .expect("integration Proxmox URL and credentials are valid");
    if let Ok(path) = std::env::var("PROXMOX_TEST_CA_CERT") {
        let certificate = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read PROXMOX_TEST_CA_CERT '{path}': {error}"));
        client_config = client_config.with_root_certificate_pem(certificate);
    }
    if let Ok(connect_ip) = std::env::var("PROXMOX_TEST_CONNECT_IP") {
        let base_url = Url::parse(&config.proxmox_base_url)
            .expect("PROXMOX_TEST_BASE_URL must be a valid URL");
        let hostname = base_url
            .host_str()
            .expect("PROXMOX_TEST_BASE_URL must contain a hostname");
        let port = base_url.port_or_known_default().unwrap_or(8006);
        let ip = connect_ip.parse::<IpAddr>().unwrap_or_else(|error| {
            panic!("PROXMOX_TEST_CONNECT_IP must be an IP address: {error}")
        });
        client_config = client_config.with_connect_override(hostname, SocketAddr::new(ip, port));
    }
    let client = ProxmoxClient::new(client_config).expect("Proxmox client can be built");
    let status = client
        .get_lxc_status(&config.node_name, config.vmid)
        .await
        .expect("dedicated integration LXC is reachable");
    assert_eq!(status.vmid, config.vmid);
}
