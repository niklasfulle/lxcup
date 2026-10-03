use std::time::Duration;

use chrono::Utc;
use lxcup_persistence::WorkerHeartbeatRepository;

pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
}

pub(crate) async fn is_available(client: &reqwest::Client, base_url: &str) -> bool {
    let manifest_url = format!(
        "{}/agent/{}/manifest.json",
        base_url.trim_end_matches('/'),
        lxcup_core::VERSION
    );
    let Ok(response) = client.get(manifest_url).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(manifest) = response.json::<serde_json::Value>().await else {
        return false;
    };
    supports_manifest(&manifest)
}

fn supports_manifest(manifest: &serde_json::Value) -> bool {
    manifest.get("version").and_then(serde_json::Value::as_str) == Some(lxcup_core::VERSION)
        && manifest
            .get("artifacts")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|artifacts| {
                ["linux-amd64", "linux-arm64", "windows-amd64"]
                    .iter()
                    .all(|platform| {
                        artifacts.iter().any(|artifact| {
                            artifact.get("platform").and_then(serde_json::Value::as_str)
                                == Some(*platform)
                                && artifact
                                    .get("file")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some()
                                && artifact
                                    .get("sha256")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some()
                        })
                    })
            })
}

pub(crate) async fn report_availability(
    repository: WorkerHeartbeatRepository,
    worker_name: String,
    worker_version: String,
    client: reqwest::Client,
    base_url: String,
) {
    let mut available = false;
    let mut checked_at = Utc::now();
    let mut next_probe = tokio::time::Instant::now();
    let mut previous_status = None;
    loop {
        if tokio::time::Instant::now() >= next_probe {
            available = is_available(&client, &base_url).await;
            checked_at = Utc::now();
            match (previous_status, available) {
                (None, false) | (Some(true), false) => tracing::warn!(
                    "artifact store is unavailable; the worker will retry its health check"
                ),
                (Some(false), true) => tracing::info!("artifact store is available again"),
                _ => {}
            }
            previous_status = Some(available);
            next_probe = tokio::time::Instant::now() + PROBE_INTERVAL;
        }
        if let Err(error) = repository
            .record(&worker_name, &worker_version, available, checked_at)
            .await
        {
            tracing::error!(?error, "worker heartbeat failed");
        }
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_probe_requires_current_version_and_all_supported_artifacts() {
        let manifest = serde_json::json!({
            "version": lxcup_core::VERSION,
            "artifacts": [
                { "platform": "linux-amd64", "file": "agent-amd64", "sha256": "digest" },
                { "platform": "linux-arm64", "file": "agent-arm64", "sha256": "digest" },
                { "platform": "windows-amd64", "file": "agent.exe", "sha256": "digest" }
            ]
        });
        assert!(supports_manifest(&manifest));

        let wrong_version = serde_json::json!({
            "version": "0.0.0",
            "artifacts": [
                { "platform": "linux-amd64", "file": "agent-amd64", "sha256": "digest" },
                { "platform": "linux-arm64", "file": "agent-arm64", "sha256": "digest" },
                { "platform": "windows-amd64", "file": "agent.exe", "sha256": "digest" }
            ]
        });
        assert!(!supports_manifest(&wrong_version));

        let missing_linux_artifact = serde_json::json!({
            "version": lxcup_core::VERSION,
            "artifacts": [{ "platform": "windows-amd64", "file": "agent.exe", "sha256": "digest" }]
        });
        assert!(!supports_manifest(&missing_linux_artifact));

        let missing_arm64_artifact = serde_json::json!({
            "version": lxcup_core::VERSION,
            "artifacts": [{ "platform": "linux-amd64", "file": "agent-amd64", "sha256": "digest" }]
        });
        assert!(!supports_manifest(&missing_arm64_artifact));

        let missing_windows_artifact = serde_json::json!({
            "version": lxcup_core::VERSION,
            "artifacts": [
                { "platform": "linux-amd64", "file": "agent-amd64", "sha256": "digest" },
                { "platform": "linux-arm64", "file": "agent-arm64", "sha256": "digest" }
            ]
        });
        assert!(!supports_manifest(&missing_windows_artifact));
    }
}
