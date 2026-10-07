use std::{
    ffi::OsString,
    fmt::Display,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{process::Command as TokioCommand, sync::oneshot};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "lxcup-agent";
const MAX_AGENT_ARTIFACT_BYTES: usize = 128 * 1024 * 1024;

define_windows_service!(ffi_service_main, service_main);

pub(super) fn start() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

pub(super) fn spawn_agent_update(job_id: uuid::Uuid, version: &str) -> std::io::Result<()> {
    if !valid_version(version) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent update version is invalid",
        ));
    }
    Command::new(std::env::current_exe()?)
        .args(["--apply-agent-update", &job_id.to_string(), version])
        .spawn()
        .map(|_| ())
}

pub(super) async fn run_agent_update_helper(job_id: uuid::Uuid, version: String) -> i32 {
    lxcup_observability::init("lxcup-agent-updater");
    let config = match super::startup_config_from_environment() {
        Ok(config) if config.heartbeat.is_some() => config,
        Ok(_) => {
            tracing::error!("Windows agent updater requires a configured controller target");
            return 2;
        }
        Err(error) => {
            tracing::error!(%error, "Windows agent updater configuration is invalid");
            return 2;
        }
    };
    let Some((controller, _)) = config.heartbeat.as_ref() else {
        return 2;
    };
    let controller = controller.clone();
    let result = update_agent_binary(&controller, &config.token, &version).await;
    let response = match &result {
        Ok(()) => lxcup_agent::AgentCommandResponse {
            request_id: uuid::Uuid::new_v4(),
            success: true,
            exit_code: 0,
            stdout: format!("Agent updated to version {version} from the artifact store."),
            stderr: String::new(),
            reboot_required: false,
            duration_ms: 0,
        },
        Err(error) => lxcup_agent::AgentCommandResponse {
            request_id: uuid::Uuid::new_v4(),
            success: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("Agent update failed: {error}"),
            reboot_required: false,
            duration_ms: 0,
        },
    };
    report_update_result(&controller, &config.token, job_id, response).await;
    if result.is_ok() { 0 } else { 1 }
}

#[derive(Deserialize)]
struct ArtifactManifest {
    version: String,
    artifacts: Vec<ArtifactEntry>,
}

#[derive(Deserialize)]
struct ArtifactEntry {
    platform: String,
    file: String,
    sha256: String,
}

async fn update_agent_binary(controller: &str, token: &str, version: &str) -> Result<(), String> {
    if !valid_version(version) {
        return Err("requested version is invalid".to_owned());
    }
    let controller = validated_controller_url(controller)?;
    let base = format!(
        "{}/agent/{version}",
        controller.as_str().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "could not create artifact client")?;
    let (bytes, expected_hash) =
        download_verified_windows_artifact(&client, &base, version).await?;

    let install_directory = Path::new(r"C:\Program Files\lxcup");
    fs::create_dir_all(install_directory).map_err(|_| "agent install directory is unavailable")?;
    let current_path = std::env::current_exe().map_err(|_| "current agent path is unavailable")?;
    if current_path == versioned_agent_path(install_directory, version) {
        return Ok(());
    }
    let next_path = versioned_agent_path(install_directory, version);
    let staged_path = install_directory.join(format!("lxcup-agent-{version}.staged.exe"));
    fs::write(&staged_path, &bytes).map_err(|_| "verified agent could not be staged")?;
    if let Err(error) = validate_file(&staged_path, &expected_hash) {
        let _ = fs::remove_file(&staged_path);
        return Err(error);
    }
    if next_path.exists() {
        fs::remove_file(&next_path).map_err(|_| "stale staged agent could not be replaced")?;
    }
    fs::rename(&staged_path, &next_path).map_err(|_| "verified agent could not be activated")?;

    restart_service(&current_path, &next_path, token).await
}

async fn download_verified_windows_artifact(
    client: &reqwest::Client,
    base: &str,
    version: &str,
) -> Result<(Vec<u8>, String), String> {
    let manifest = client
        .get(format!("{base}/manifest.json"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "agent manifest could not be downloaded")?
        .json::<ArtifactManifest>()
        .await
        .map_err(|_| "agent manifest is invalid")?;
    let expected_hash = windows_artifact_hash(&manifest, version)?;

    let response = client
        .get(format!("{base}/windows-amd64.exe"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "Windows agent artifact could not be downloaded")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AGENT_ARTIFACT_BYTES as u64)
    {
        return Err("Windows agent artifact exceeds the size limit".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Windows agent artifact could not be read")?;
    if bytes.is_empty() || bytes.len() > MAX_AGENT_ARTIFACT_BYTES {
        return Err("Windows agent artifact has an invalid size".to_owned());
    }
    let actual_hash = format!("{:x}", Sha256::digest(&bytes));
    if !actual_hash.eq_ignore_ascii_case(&expected_hash) {
        return Err("Windows agent artifact checksum does not match the manifest".to_owned());
    }
    validate_windows_amd64_pe(&bytes)?;
    Ok((bytes.to_vec(), expected_hash))
}

fn windows_artifact_hash(manifest: &ArtifactManifest, version: &str) -> Result<String, String> {
    if manifest.version != version {
        return Err("artifact manifest version does not match the requested version".to_owned());
    }
    let mut matches = manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.platform == "windows-amd64");
    let artifact = matches.next().ok_or("Windows amd64 artifact is missing")?;
    if matches.next().is_some()
        || artifact.file != "windows-amd64.exe"
        || !is_sha256(&artifact.sha256)
    {
        return Err("Windows artifact manifest entry is invalid".to_owned());
    }
    Ok(artifact.sha256.clone())
}

fn validated_controller_url(controller: &str) -> Result<reqwest::Url, String> {
    let controller = reqwest::Url::parse(controller).map_err(|_| "controller URL is invalid")?;
    let is_loopback = controller
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
        || controller.host_str() == Some("localhost");
    let secure_transport =
        controller.scheme() == "https" || (controller.scheme() == "http" && is_loopback);
    let has_credentials = !controller.username().is_empty() || controller.password().is_some();
    let has_url_suffix = controller.query().is_some() || controller.fragment().is_some();
    if !secure_transport || has_credentials || has_url_suffix {
        return Err("controller URL must use validated HTTPS".to_owned());
    }
    Ok(controller)
}

fn versioned_agent_path(directory: &Path, version: &str) -> std::path::PathBuf {
    directory.join(format!("lxcup-agent-{version}.exe"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_version(value: &str) -> bool {
    let (release, prerelease) = value.split_once('-').unwrap_or((value, ""));
    let mut parts = release.split('.');
    value.len() <= 50
        && (0..3).all(|_| {
            parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
        && parts.next().is_none()
        && (prerelease.is_empty()
            || prerelease
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
}

fn validate_windows_amd64_pe(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 0x100 || bytes.get(0..2) != Some(b"MZ") {
        return Err("artifact is not a valid Windows executable".to_owned());
    }
    let pe_offset = u32::from_le_bytes(
        bytes[0x3c..0x40]
            .try_into()
            .map_err(|_| "invalid PE header")?,
    ) as usize;
    let pe_header = bytes
        .get(pe_offset..pe_offset.saturating_add(26))
        .ok_or("invalid PE header")?;
    let signature = u32::from_le_bytes(
        pe_header[0..4]
            .try_into()
            .map_err(|_| "invalid PE signature")?,
    );
    let machine = u16::from_le_bytes(
        pe_header[4..6]
            .try_into()
            .map_err(|_| "invalid PE machine")?,
    );
    let magic = u16::from_le_bytes(
        pe_header[24..26]
            .try_into()
            .map_err(|_| "invalid PE format")?,
    );
    if signature != 0x0000_4550 || machine != 0x8664 || magic != 0x020b {
        return Err("artifact is not a PE32+ amd64 executable".to_owned());
    }
    Ok(())
}

fn validate_file(path: &Path, expected_hash: &str) -> Result<(), String> {
    let file = fs::File::open(path).map_err(|_| "staged agent could not be verified")?;
    let mut contents = Vec::new();
    file.take((MAX_AGENT_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|_| "staged agent could not be read")?;
    if contents.len() > MAX_AGENT_ARTIFACT_BYTES
        || !format!("{:x}", Sha256::digest(&contents)).eq_ignore_ascii_case(expected_hash)
    {
        return Err("staged agent checksum does not match".to_owned());
    }
    validate_windows_amd64_pe(&contents)
}

async fn restart_service(current_path: &Path, next_path: &Path, token: &str) -> Result<(), String> {
    run_sc(&["stop", SERVICE_NAME]).await?;
    wait_service_state(false).await?;
    let next = format!("\"{}\"", next_path.to_string_lossy());
    let previous = format!("\"{}\"", current_path.to_string_lossy());
    let changed = run_sc(&[
        "config",
        SERVICE_NAME,
        "binPath=",
        &next,
        "start=",
        "auto",
        "obj=",
        "LocalSystem",
    ])
    .await;
    if let Err(error) = changed {
        let _ = restore_service(&previous).await;
        return Err(error);
    }
    let start_result = run_sc(&["start", SERVICE_NAME]).await;
    if start_result.is_ok() && wait_for_agent_version(next_path, token, 30).await {
        return Ok(());
    }
    let _ = run_sc(&["stop", SERVICE_NAME]).await;
    let _ = wait_service_state(false).await;
    let rollback = restore_service(&previous).await;
    if rollback.is_err() {
        return Err("new agent failed health verification and service rollback failed".to_owned());
    }
    Err("new agent failed health verification; previous agent was restored".to_owned())
}

async fn restore_service(previous_path: &str) -> Result<(), String> {
    run_sc(&[
        "config",
        SERVICE_NAME,
        "binPath=",
        previous_path,
        "start=",
        "auto",
        "obj=",
        "LocalSystem",
    ])
    .await?;
    run_sc(&["start", SERVICE_NAME]).await
}

async fn run_sc(arguments: &[&str]) -> Result<(), String> {
    let output = TokioCommand::new("sc.exe")
        .args(arguments)
        .output()
        .await
        .map_err(|_| "Windows service manager could not be reached")?;
    if output.status.success() {
        Ok(())
    } else {
        Err("Windows service configuration or restart failed".to_owned())
    }
}

async fn wait_service_state(running: bool) -> Result<(), String> {
    for _ in 0..30 {
        let output = TokioCommand::new("sc.exe")
            .args(["query", SERVICE_NAME])
            .output()
            .await
            .map_err(|_| "Windows service state could not be checked")?;
        let text = String::from_utf8_lossy(&output.stdout);
        let expected = if running { "4" } else { "1" };
        if text.lines().any(|line| {
            line.contains("STATE")
                && line
                    .split_whitespace()
                    .any(|part| part.trim_end_matches(':') == expected)
        }) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("Windows agent service did not reach the requested state".to_owned())
}

async fn wait_for_agent_version(path: &Path, token: &str, attempts: u8) -> bool {
    let expected = path
        .file_stem()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("lxcup-agent-"));
    let Some(expected) = expected else {
        return false;
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    for _ in 0..attempts {
        let endpoint = "http://127.0.0.1:8090/health";
        if let Ok(response) = client.get(endpoint).bearer_auth(token).send().await {
            if let Ok(health) = response.json::<lxcup_agent::AgentHealth>().await {
                if health.info.version == expected {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

async fn report_update_result(
    controller: &str,
    token: &str,
    job_id: uuid::Uuid,
    response: lxcup_agent::AgentCommandResponse,
) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    else {
        return;
    };
    let endpoint = format!(
        "{}/api/v1/agents/workflows/{job_id}/result",
        controller.trim_end_matches('/')
    );
    loop {
        match client
            .post(&endpoint)
            .bearer_auth(token)
            .json(&lxcup_agent::AgentWorkflowResult {
                response: response.clone(),
            })
            .send()
            .await
        {
            Ok(result) if result.status().is_success() => return,
            Ok(result) => {
                tracing::warn!(job_id = %job_id, status = %result.status(), "agent update result report was rejected; retrying")
            }
            Err(error) => {
                tracing::warn!(job_id = %job_id, %error, "agent update result report failed; retrying")
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        record_startup_failure("service entry point", &error);
        tracing::error!(?error, "Windows service stopped after an internal error");
    }
}

pub(super) fn record_startup_failure(stage: &str, error: &impl Display) {
    let log_path = Path::new(r"C:\ProgramData\lxcup\agent-startup.log");
    if let Err(log_error) = write_startup_failure(log_path, stage, error) {
        tracing::error!(
            ?log_error,
            stage,
            "could not persist Windows agent startup diagnostics"
        );
    }
}

fn write_startup_failure(path: &Path, stage: &str, error: &impl Display) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    writeln!(file, "{stage}: {error}")
}

fn run_service() -> windows_service::Result<()> {
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let shutdown_sender = Arc::new(Mutex::new(Some(shutdown_sender)));
    let event_sender = Arc::clone(&shutdown_sender);
    let status = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop => {
            if let Ok(mut sender) = event_sender.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    status.set_service_status(start_pending_status())?;

    let config = match super::startup_config_from_environment() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "Windows agent configuration is invalid");
            return set_stopped(&status, ServiceExitCode::ServiceSpecific(1));
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(?error, "could not create Windows agent runtime");
            return set_stopped(&status, ServiceExitCode::ServiceSpecific(2));
        }
    };

    status.set_service_status(running_status())?;

    let result = runtime.block_on(super::run(config, async move {
        let _ = shutdown_receiver.await;
    }));
    if let Err(error) = &result {
        tracing::error!(?error, "Windows agent runtime failed");
    }
    status.set_service_status(stopped_status(result.as_ref().err().map_or(
        ServiceExitCode::NO_ERROR,
        |error| {
            ServiceExitCode::Win32(
                error
                    .raw_os_error()
                    .and_then(|code| u32::try_from(code).ok())
                    .unwrap_or(3),
            )
        },
    )))
}

fn start_pending_status() -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::NO_ERROR,
        checkpoint: 1,
        wait_hint: Duration::from_secs(30),
        process_id: None,
    }
}

fn running_status() -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::NO_ERROR,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

fn stopped_status(exit_code: ServiceExitCode) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

fn set_stopped(
    status: &windows_service::service_control_handler::ServiceStatusHandle,
    exit_code: ServiceExitCode,
) -> windows_service::Result<()> {
    status.set_service_status(stopped_status(exit_code))
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactEntry, ArtifactManifest, download_verified_windows_artifact, is_sha256,
        running_status, spawn_agent_update, start_pending_status, stopped_status,
        update_agent_binary, valid_version, validate_file, validate_windows_amd64_pe,
        validated_controller_url, versioned_agent_path, wait_for_agent_version,
        windows_artifact_hash, write_startup_failure,
    };
    use axum::{Json, Router, routing::get};
    use sha2::Digest as _;
    use windows_service::service::{ServiceControlAccept, ServiceExitCode, ServiceState};

    #[test]
    fn service_reports_pending_before_running_and_preserves_failure_exit_codes() {
        let pending = start_pending_status();
        assert_eq!(pending.current_state, ServiceState::StartPending);
        assert!(pending.controls_accepted.is_empty());
        assert!(pending.wait_hint > std::time::Duration::ZERO);

        let running = running_status();
        assert_eq!(running.current_state, ServiceState::Running);
        assert_eq!(running.controls_accepted, ServiceControlAccept::STOP);

        let stopped = stopped_status(ServiceExitCode::Win32(87));
        assert_eq!(stopped.current_state, ServiceState::Stopped);
        assert_eq!(stopped.exit_code, ServiceExitCode::Win32(87));
    }

    #[test]
    fn startup_diagnostics_capture_stage_and_error_without_environment_values() {
        let path =
            std::env::temp_dir().join(format!("lxcup-agent-startup-{}.log", uuid::Uuid::new_v4()));
        write_startup_failure(&path, "service dispatcher", &"OS error 87").unwrap();
        let diagnostic = std::fs::read_to_string(&path).unwrap();
        assert_eq!(diagnostic, "service dispatcher: OS error 87\n");
        std::fs::remove_file(path).unwrap();
        assert!(
            write_startup_failure(&std::env::temp_dir(), "service dispatcher", &"OS error 5")
                .is_err()
        );
    }

    #[test]
    fn agent_versions_and_artifact_hashes_are_strictly_validated() {
        for version in ["0.6.0", "1.2.3-rc.1", "123.0.0"] {
            assert!(valid_version(version), "{version} should be valid");
        }
        for version in ["", "1.2", "1.2.3.4", "1..3", "1.2.3/evil", "1.2.3+meta"] {
            assert!(!valid_version(version), "{version} should be rejected");
        }
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"g".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));

        let manifest = ArtifactManifest {
            version: "0.6.0".to_owned(),
            artifacts: vec![ArtifactEntry {
                platform: "windows-amd64".to_owned(),
                file: "windows-amd64.exe".to_owned(),
                sha256: "a".repeat(64),
            }],
        };
        assert_eq!(
            windows_artifact_hash(&manifest, "0.6.0").unwrap(),
            "a".repeat(64)
        );
        assert!(windows_artifact_hash(&manifest, "0.5.0").is_err());
        let mut duplicate = manifest;
        duplicate.artifacts.push(ArtifactEntry {
            platform: "windows-amd64".to_owned(),
            file: "windows-amd64.exe".to_owned(),
            sha256: "a".repeat(64),
        });
        assert!(windows_artifact_hash(&duplicate, "0.6.0").is_err());
    }

    #[test]
    fn updater_controller_url_requires_https_except_for_loopback() {
        assert!(validated_controller_url("https://controller.example").is_ok());
        assert!(validated_controller_url("http://127.0.0.1:8080").is_ok());
        assert!(validated_controller_url("http://controller.example").is_err());
        assert!(validated_controller_url("https://user:secret@controller.example").is_err());
        assert!(validated_controller_url("https://controller.example/?token=x").is_err());
        assert!(validated_controller_url("https://controller.example/#fragment").is_err());
    }

    #[test]
    fn updater_accepts_only_amd64_pe32_plus_and_verified_files() {
        let mut executable = vec![0_u8; 0x100];
        executable[0..2].copy_from_slice(b"MZ");
        executable[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        executable[0x80..0x84].copy_from_slice(&0x0000_4550_u32.to_le_bytes());
        executable[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        executable[0x98..0x9a].copy_from_slice(&0x020b_u16.to_le_bytes());
        assert!(validate_windows_amd64_pe(&executable).is_ok());

        let mut wrong_architecture = executable.clone();
        wrong_architecture[0x84..0x86].copy_from_slice(&0x014c_u16.to_le_bytes());
        assert!(validate_windows_amd64_pe(&wrong_architecture).is_err());
        assert!(validate_windows_amd64_pe(b"not an executable").is_err());
        assert!(validate_windows_amd64_pe(&[0_u8; 0x100]).is_err());
        let mut out_of_bounds_header = executable.clone();
        out_of_bounds_header[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(validate_windows_amd64_pe(&out_of_bounds_header).is_err());

        let path = std::env::temp_dir().join(format!("lxcup-agent-{}.exe", uuid::Uuid::new_v4()));
        std::fs::write(&path, &executable).unwrap();
        let expected_hash = format!("{:x}", sha2::Sha256::digest(&executable));
        assert!(validate_file(&path, &expected_hash).is_ok());
        assert!(validate_file(&path, &"0".repeat(64)).is_err());
        assert!(validate_file(&path.with_extension("missing"), &expected_hash).is_err());
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            versioned_agent_path(std::path::Path::new("agents"), "0.6.0"),
            std::path::Path::new("agents/lxcup-agent-0.6.0.exe")
        );
    }

    #[tokio::test]
    async fn updater_rejects_invalid_versions_before_starting_or_downloading() {
        let error = update_agent_binary("not a URL", "token", "../0.6.0")
            .await
            .unwrap_err();
        assert_eq!(error, "requested version is invalid");

        let spawn_error = spawn_agent_update(uuid::Uuid::new_v4(), "../0.6.0").unwrap_err();
        assert_eq!(spawn_error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!wait_for_agent_version(std::path::Path::new("agent.exe"), "token", 0).await);
    }

    #[tokio::test]
    async fn updater_download_checks_manifest_digest_and_executable_format() {
        let mut executable = vec![0_u8; 0x100];
        executable[0..2].copy_from_slice(b"MZ");
        executable[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        executable[0x80..0x84].copy_from_slice(&0x0000_4550_u32.to_le_bytes());
        executable[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        executable[0x98..0x9a].copy_from_slice(&0x020b_u16.to_le_bytes());
        let digest = format!("{:x}", sha2::Sha256::digest(&executable));
        let manifest = serde_json::json!({
            "version": "0.6.0",
            "artifacts": [{
                "platform": "windows-amd64",
                "file": "windows-amd64.exe",
                "sha256": digest
            }]
        });
        let manifest_for_route = manifest.clone();
        let bytes_for_route = executable.clone();
        let app = Router::new()
            .route(
                "/agent/0.6.0/manifest.json",
                get(move || async move { Json(manifest_for_route) }),
            )
            .route(
                "/agent/0.6.0/windows-amd64.exe",
                get(move || async move { bytes_for_route.clone() }),
            )
            .route(
                "/agent/0.5.0/manifest.json",
                get(move || async move { Json(manifest) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();

        let (downloaded, downloaded_digest) =
            download_verified_windows_artifact(&client, &format!("{base}/agent/0.6.0"), "0.6.0")
                .await
                .expect("valid amd64 artifact should pass manifest and binary verification");
        assert_eq!(downloaded, executable);
        assert_eq!(
            downloaded_digest,
            format!("{:x}", sha2::Sha256::digest(&downloaded))
        );
        assert!(
            download_verified_windows_artifact(&client, &format!("{base}/agent/0.5.0"), "0.5.0",)
                .await
                .is_err()
        );

        server.abort();
    }
}
