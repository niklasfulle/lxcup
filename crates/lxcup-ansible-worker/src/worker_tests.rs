use super::*;
use lxcup_ansible::{AnsibleJobRequest, ExecutionMode};
use lxcup_core::{ActorRole, ResourceLifecycle, SecretId, SecretScope};
use lxcup_secrets::CreateSecret;
use std::{
    process::{ExitStatus, Output},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

async fn isolated_repositories() -> Option<(Database, sqlx::PgPool, String)> {
    let database_url = std::env::var("DATABASE_TEST_URL").ok()?;
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .ok()?;
    let schema = format!("worker_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .ok()?;
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-csearch_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        3,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        None,
    )
    .ok()?;
    let database = Database::connect(&config).await.ok()?;
    database.migrate().await.ok()?;
    Some((database, admin, schema))
}

async fn drop_test_schema(database: Database, admin: sqlx::PgPool, schema: &str) {
    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

fn worker_output(success: bool, stdout: &str, stderr: &str) -> Output {
    let status = test_exit_status(success);
    Output {
        status,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[cfg(unix)]
fn test_exit_status(success: bool) -> ExitStatus {
    ExitStatus::from_raw(if success { 0 } else { 1 })
}

#[cfg(windows)]
fn test_exit_status(success: bool) -> ExitStatus {
    ExitStatus::from_raw(if success { 0 } else { 1 })
}

fn package_update_job(target: &Target, key: &str) -> AnsibleJob {
    AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::UpdatePackages,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: lxcup_ansible::ExecutionMode::Apply,
        parameters: AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()],
        },
        secret_refs: vec![target.agent_secret_ref],
        idempotency_key: key.to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap()
}

fn health_job(target: &Target, key: &str) -> AnsibleJob {
    AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: key.to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap()
}

async fn artifact_server(
    manifest: serde_json::Value,
    binary: Vec<u8>,
    requests: usize,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for index in 0..requests {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let body = if index == 0 {
                serde_json::to_vec(&manifest).unwrap()
            } else {
                binary.clone()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });
    (format!("http://{address}"), server)
}

fn runtime_with_secrets(
    path: &Path,
) -> (
    Runtime,
    lxcup_core::SecretId,
    lxcup_core::SecretId,
    lxcup_core::SecretId,
) {
    let store =
        EncryptedFileSecretStore::new(path, SecretMasterKey::from_bytes([7; 32]), []).unwrap();
    let credential = store
        .create(CreateSecret {
            name: "worker-credential".to_owned(),
            kind: SecretKind::SshPassword,
            scope: SecretScope::Global,
            value: SecretValue::new("ssh-password").unwrap(),
        })
        .unwrap();
    let agent = store
        .create(CreateSecret {
            name: "worker-agent".to_owned(),
            kind: SecretKind::AgentToken,
            scope: SecretScope::Global,
            value: SecretValue::new("agent-token").unwrap(),
        })
        .unwrap();
    let known_hosts = store
        .create(CreateSecret {
            name: "worker-known-hosts".to_owned(),
            kind: SecretKind::SshKnownHosts,
            scope: SecretScope::Global,
            value: SecretValue::new("192.0.2.20 ssh-ed25519 AAAA").unwrap(),
        })
        .unwrap();
    (
        Runtime {
            secrets: store,
            artifacts: "http://127.0.0.1:1".to_owned(),
            artifact_probe: reqwest::Client::new(),
            user: "lxcup".to_owned(),
            controller_url: None,
        },
        credential.metadata.id,
        agent.metadata.id,
        known_hosts.metadata.id,
    )
}

#[test]
fn playbook_registry_is_explicit_for_supported_operations() {
    assert_eq!(
        playbook(AnsibleOperation::DeployAgent, TargetKind::Lxc),
        Some("playbooks/agent-linux.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::UpdateAgent, TargetKind::WindowsServer),
        Some("playbooks/agent-windows.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::RepairAgent, TargetKind::LinuxServer),
        Some("playbooks/agent-linux-repair.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::UpdatePackages, TargetKind::WindowsServer),
        Some("playbooks/packages-windows.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::UpdatePackages, TargetKind::Lxc),
        Some("playbooks/packages-linux.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::HealthCheck, TargetKind::Lxc),
        Some("playbooks/health-check-linux.yml")
    );
    assert_eq!(
        playbook(
            AnsibleOperation::CollectPackageInventory,
            TargetKind::LinuxServer
        ),
        Some("playbooks/package-inventory-linux.yml")
    );
    assert_eq!(
        playbook(
            AnsibleOperation::CollectPackageInventory,
            TargetKind::WindowsServer
        ),
        Some("playbooks/package-inventory-windows.yml")
    );
    assert_eq!(
        playbook(AnsibleOperation::ConfigureTarget, TargetKind::Lxc),
        None
    );
}

#[test]
fn worker_logs_redact_all_secret_occurrences_and_ignore_empty_values() {
    let output = redact_output(
        "password=abc token=xyz abc",
        &[Some("abc"), Some("xyz"), Some("")],
    );
    assert_eq!(output, "password=[REDACTED] token=[REDACTED] [REDACTED]");
}

#[test]
fn plan_diff_redacts_secret_values_before_preview_logging() {
    let raw_diff = "--- before\n+++ after\n+token=agent-secret-123";
    let preview = redact_output(raw_diff, &[Some("agent-secret-123"), None]);

    assert!(!preview.contains("agent-secret-123"));
    assert!(preview.contains("token=[REDACTED]"));
}

#[test]
fn worker_classifies_common_ansible_failures_and_change_summary() {
    assert_eq!(
        classify_playbook_failure("fatal: Invalid/incorrect password"),
        JobFailureCode::InvalidCredentials
    );
    assert_eq!(
        classify_playbook_failure("target UNREACHABLE! unreachable: true"),
        JobFailureCode::Unreachable
    );
    assert_eq!(
        classify_playbook_failure("Host key verification failed"),
        JobFailureCode::HostKeyChanged
    );
    assert_eq!(
        classify_playbook_failure("role was not found"),
        JobFailureCode::PlaybookFailed
    );
    assert!(!playbook_changed("PLAY RECAP changed=0 failed=0"));
    assert!(playbook_changed("PLAY RECAP changed=1 failed=0"));
    assert!(playbook_changed(
        "PLAY RECAP\nhost-a : changed=2 failed=0\nhost-b : changed=0 failed=0"
    ));
}

#[test]
fn apply_audit_events_report_actual_changed_tasks_and_result() {
    let output = "PLAY [target]\nTASK [Install runtime]\nchanged: [target]\nTASK [Validate agent]\nok: [target]\nTASK [Conditional restart]\nskipping: [target]\nPLAY RECAP\ntarget : ok=1 changed=1 failed=0";
    assert_eq!(
        finished_task_results(output),
        vec![
            ("Install runtime".to_owned(), true),
            ("Validate agent".to_owned(), false),
        ]
    );
    assert!(apply_summary(output, true).contains("1 Task(s) haben Änderungen vorgenommen"));
    assert!(apply_summary(output, false).contains("Apply fehlgeschlagen"));
}

#[test]
fn check_mode_uses_ansible_check_flag_and_never_falls_through_to_apply() {
    for operation in [
        AnsibleOperation::DeployAgent,
        AnsibleOperation::UpdateAgent,
        AnsibleOperation::RepairAgent,
        AnsibleOperation::ConfigureTarget,
    ] {
        assert_eq!(
            ansible_mode_args(operation, ExecutionMode::Check),
            Ok(["--check"].as_slice())
        );
        assert_eq!(
            ansible_mode_args(operation, ExecutionMode::Apply),
            Ok([].as_slice())
        );
    }
    for operation in [
        AnsibleOperation::DeployAgent,
        AnsibleOperation::UpdateAgent,
        AnsibleOperation::RepairAgent,
    ] {
        assert_eq!(
            ansible_mode_args(operation, ExecutionMode::Plan),
            Ok(["--check", "--diff"].as_slice())
        );
    }
    assert_eq!(
        ansible_mode_args(AnsibleOperation::UpdatePackages, ExecutionMode::Plan),
        Ok(["--diff"].as_slice())
    );
    assert_eq!(
        ansible_mode_args(AnsibleOperation::ConfigureTarget, ExecutionMode::Plan),
        Err(JobFailureCode::PlaybookFailed)
    );
    for operation in [
        AnsibleOperation::DeployAgent,
        AnsibleOperation::UpdateAgent,
        AnsibleOperation::RepairAgent,
    ] {
        assert_eq!(
            ansible_mode_args(operation, ExecutionMode::Reconcile),
            Ok(["--check", "--diff"].as_slice())
        );
    }
    assert_eq!(
        ansible_mode_args(AnsibleOperation::UpdatePackages, ExecutionMode::Reconcile),
        Ok(["--diff"].as_slice())
    );
    assert_eq!(
        ansible_mode_args(AnsibleOperation::HealthCheck, ExecutionMode::Reconcile),
        Err(JobFailureCode::PlaybookFailed)
    );
    for operation in [
        AnsibleOperation::HealthCheck,
        AnsibleOperation::CollectPackageInventory,
    ] {
        assert_eq!(
            ansible_mode_args(operation, ExecutionMode::Check),
            Ok([].as_slice())
        );
    }
    assert_eq!(
        ansible_mode_args(AnsibleOperation::UpdatePackages, ExecutionMode::Check),
        Err(JobFailureCode::PlaybookFailed)
    );
}

#[test]
fn check_mode_summary_marks_skipped_and_missing_results_unverifiable() {
    assert!(check_mode_summary("skipping: [target]\nPLAY RECAP").contains("nicht prüfbar"));
    assert!(check_mode_summary("PLAY [target]").contains("keine vollständige"));
    assert!(
        check_mode_summary("PLAY RECAP\ntarget : ok=0 changed=0 failed=1")
            .contains("fehlgeschlagen")
    );
    assert!(check_mode_summary("PLAY RECAP changed=1").contains("nichts angewendet"));
    assert!(check_mode_summary("PLAY RECAP changed=0").contains("keine Änderungen"));
}

#[test]
fn reconciliation_summary_classifies_the_observed_target_state() {
    assert_eq!(
        reconciliation_decision("PLAY RECAP\ntarget : ok=3 changed=0 failed=0 skipped=0"),
        ReconciliationDecision::Completed
    );
    assert_eq!(
        reconciliation_decision("PLAY RECAP\ntarget : ok=2 changed=1 failed=0 skipped=0"),
        ReconciliationDecision::ChangesRemain
    );
    assert_eq!(
        reconciliation_decision("PLAY RECAP\ntarget : ok=2 changed=0 failed=1 skipped=0"),
        ReconciliationDecision::ManualReview
    );
    assert_eq!(
        reconciliation_decision("PLAY RECAP\ntarget : ok=2 changed=0 failed=0 skipped=1"),
        ReconciliationDecision::ManualReview
    );
    assert_eq!(
        reconciliation_decision("PLAY [target]"),
        ReconciliationDecision::ManualReview
    );
    assert!(
        reconciliation_summary(ReconciliationDecision::Completed)
            .contains("hat selbst nichts geändert")
    );
    assert!(
        reconciliation_summary(ReconciliationDecision::ChangesRemain)
            .contains("bestätige einen neuen Apply ausdrücklich")
    );
}

#[test]
fn plan_summary_reports_predicted_changes_without_claiming_apply() {
    assert!(plan_summary("PLAY RECAP\ntarget : ok=4 changed=2 failed=0").contains("2 Änderung"));
    assert!(
        plan_summary("PLAY RECAP\ntarget : ok=4 changed=0 failed=0").contains("keine Änderungen")
    );
    assert!(
        plan_summary("PLAY RECAP\ntarget : ok=0 changed=0 failed=1")
            .contains("Vorschau fehlgeschlagen")
    );
    assert!(plan_summary("skipping: [target]\nPLAY RECAP").contains("teilweise nicht prüfbar"));
    assert!(plan_summary("PLAY [target]").contains("Vorschau unvollständig"));
}

#[test]
fn mutating_playbooks_do_not_disable_ansible_check_mode() {
    let playbooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks");
    for file in [
        "agent-linux.yml",
        "agent-linux-repair.yml",
        "agent-windows.yml",
    ] {
        let contents = fs::read_to_string(playbooks.join(file)).unwrap();
        assert!(
            !contents.contains("check_mode: false"),
            "{file} disables check mode"
        );
    }
}

#[test]
fn released_agent_020_manifest_matches_binary_checksum() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/agent/0.2.0");
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(root.join("manifest.json")).expect("0.2.0 manifest must exist"),
    )
    .expect("0.2.0 manifest must be valid JSON");
    assert_eq!(manifest.version, "0.2.0");
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == "linux-amd64")
        .expect("linux artifact must be registered");
    let digest = Sha256::digest(fs::read(root.join(&artifact.file)).expect("binary exists"));
    assert_eq!(format!("{digest:x}"), artifact.sha256);
}

#[test]
fn released_agent_030_manifest_matches_binary_checksum() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/agent/0.3.1");
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(root.join("manifest.json")).expect("0.3.1 manifest must exist"),
    )
    .expect("0.3.1 manifest must be valid JSON");
    assert_eq!(manifest.version, "0.3.1");
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == "linux-amd64")
        .expect("linux artifact must be registered");
    let digest = Sha256::digest(fs::read(root.join(&artifact.file)).expect("binary exists"));
    assert_eq!(format!("{digest:x}"), artifact.sha256);
}

#[test]
fn current_agent_manifest_matches_binary_checksum() {
    let version = env!("CARGO_PKG_VERSION");
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../artifacts/agent/{version}"));
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(root.join("manifest.json")).expect("current agent manifest must exist"),
    )
    .expect("current agent manifest must be valid JSON");
    assert_eq!(manifest.version, version);
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == "linux-amd64")
        .expect("linux artifact must be registered");
    let digest = Sha256::digest(fs::read(root.join(&artifact.file)).expect("binary exists"));
    assert_eq!(format!("{digest:x}"), artifact.sha256);
}

#[test]
fn package_inventory_normalization_preserves_versions_and_metadata() {
    let packages = normalize_package_inventory(
        serde_json::json!({
            "curl": [{"version": "8.5.0-2", "arch": "amd64", "source": "apt"}],
            "zlib1g": [{"version": "1:1.2.13", "architecture": "amd64"}]
        }),
        &["curl/stable 8.6.0-1 amd64 [upgradable from: 8.5.0-2]".to_owned()],
    )
    .unwrap();

    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].name.as_str(), "curl");
    assert_eq!(packages[0].architecture.as_deref(), Some("amd64"));
    assert_eq!(
        packages[0]
            .candidate_version
            .as_ref()
            .map(|version| version.as_str()),
        Some("8.6.0-1")
    );
    assert_eq!(packages[1].version.as_str(), "1:1.2.13");
    assert_eq!(
        packages[1]
            .candidate_version
            .as_ref()
            .map(|version| version.as_str()),
        Some("1:1.2.13")
    );
    assert!(normalize_package_inventory(serde_json::json!([]), &[]).is_err());
}

#[test]
fn apt_upgrade_parser_ignores_headers_and_non_upgrade_rows() {
    assert_eq!(parse_apt_upgrade("Listing..."), None);
    assert_eq!(parse_apt_upgrade("WARNING: cache is old"), None);
    assert_eq!(
        parse_apt_upgrade("curl/stable 8.6.0-1 amd64 [upgradable from: 8.5.0-2]"),
        Some(("curl".to_owned(), "8.6.0-1".to_owned()))
    );
    assert_eq!(parse_apt_upgrade("curl/stable 8.6.0-1 amd64"), None);
}

#[test]
fn private_writes_exact_contents_for_temporary_worker_files() {
    let path = std::env::temp_dir().join(format!("lxcup-worker-test-{}", Uuid::new_v4()));
    private(&path, "inventory").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "inventory");
    fs::remove_file(path).unwrap();
}

#[test]
fn agent_variables_are_skipped_for_health_checks_and_require_a_secret_otherwise() {
    let root = std::env::temp_dir().join(format!("lxcup-worker-secrets-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let mut target = Target::new(
        "worker-test",
        TargetKind::LinuxServer,
        "192.0.2.20",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let (runtime, _credential_secret_ref, agent_secret_ref, _known_hosts_secret_ref) =
        runtime_with_secrets(&root);
    target.agent_secret_ref = agent_secret_ref;
    let health_job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "worker-health-test".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap();
    let mut vars = serde_json::json!({});
    assert!(
        prepare_agent_vars(&runtime, &health_job, &target, &mut vars)
            .unwrap()
            .is_none()
    );
    assert!(vars.get("lxcup_agent_token").is_none());

    let deploy_job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::UpdatePackages,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Apply,
        parameters: AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()],
        },
        secret_refs: vec![target.agent_secret_ref],
        idempotency_key: "worker-package-test".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap();
    let mut vars = serde_json::json!({});
    assert!(
        prepare_agent_vars(&runtime, &deploy_job, &target, &mut vars)
            .unwrap()
            .is_some()
    );
    assert_eq!(vars["lxcup_agent_token"], "agent-token");
    assert_eq!(vars["lxcup_agent_id"], target.id.as_uuid().to_string());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invocation_prepares_private_inventory_and_known_hosts_files() {
    let root = std::env::temp_dir().join(format!("lxcup-worker-invocation-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let (runtime, credential_ref, _agent_ref, known_hosts_ref) = runtime_with_secrets(&root);
    let mut target = Target::new(
        "worker-invocation",
        TargetKind::LinuxServer,
        "192.0.2.20",
        TargetTransport::Ssh,
        credential_ref,
        SecretId::new(),
    )
    .unwrap();
    target.ssh_user = Some("deploy".to_owned());
    target.ssh_known_hosts_secret_ref = Some(known_hosts_ref);
    let job = AnsibleJob::from_request(AnsibleJobRequest {
        operation: AnsibleOperation::HealthCheck,
        target: ResourceTarget::Target(target.id),
        lifecycle: ResourceLifecycle::Managed,
        mode: ExecutionMode::Check,
        parameters: AnsibleParameters::HealthCheck,
        secret_refs: Vec::new(),
        idempotency_key: "worker-invocation-test".to_owned(),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
    .unwrap();
    let context = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(prepare_invocation(&runtime, &job, &target, &root))
        .unwrap();
    let inventory = fs::read_to_string(&context.inventory).unwrap();
    let vars = fs::read_to_string(&context.vars_file).unwrap();
    assert!(inventory.contains("192.0.2.20"));
    assert!(inventory.contains("deploy"));
    assert!(inventory.contains("known_hosts"));
    assert!(vars.contains("check"));
    assert_eq!(context.credential.expose(), "ssh-password");
    assert!(context.agent_token.is_none());
    assert_eq!(
        fs::read_to_string(root.join("known_hosts")).unwrap(),
        "192.0.2.20 ssh-ed25519 AAAA"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_configuration_validates_required_values_and_normalizes_urls() {
    assert!(matches!(
        Runtime::from_values(None, None, None, None, None),
        Err("LXCUP_SECRET_STORE_DIR is missing")
    ));
    let root = std::env::temp_dir().join(format!("lxcup-worker-runtime-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    assert!(matches!(
        Runtime::from_values(Some(root.display().to_string()), None, None, None, None),
        Err("LXCUP_SECRET_MASTER_KEY must be 64 hex characters")
    ));
    let runtime = Runtime::from_values(
        Some(root.display().to_string()),
        Some(SecretMasterKey::from_bytes([8; 32])),
        Some("http://artifacts/".to_owned()),
        None,
        Some("http://controller///".to_owned()),
    )
    .unwrap();
    assert_eq!(runtime.artifacts, "http://artifacts");
    assert_eq!(runtime.user, "lxcup");
    assert_eq!(runtime.controller_url.as_deref(), Some("http://controller"));
    assert!(matches!(
        Runtime::from_values(
            Some(root.display().to_string()),
            Some(SecretMasterKey::from_bytes([8; 32])),
            None,
            Some("deploy".to_owned()),
            Some("///".to_owned()),
        ),
        Err("LXCUP_ARTIFACT_BASE_URL is missing")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_inventory_file_reader_handles_missing_invalid_and_valid_files() {
    let root = std::env::temp_dir().join(format!("lxcup-worker-packages-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let target_id = lxcup_core::TargetId::new();
    assert_eq!(
        read_package_inventory(&root, target_id),
        Err(JobFailureCode::PlaybookFailed)
    );
    fs::write(root.join("package-inventory.json"), "not-json").unwrap();
    assert_eq!(
        read_package_inventory(&root, target_id),
        Err(JobFailureCode::PlaybookFailed)
    );
    fs::write(
        root.join("package-inventory.json"),
        r#"{"packages":{"curl":[{"version":"8.5.0","arch":"amd64"}]},"upgradable":["curl/stable 8.6.0 amd64 [upgradable from: 8.5.0]"]}"#,
    )
    .unwrap();
    let snapshot = read_package_inventory(&root, target_id).unwrap();
    assert_eq!(snapshot.target_id, target_id);
    assert_eq!(snapshot.packages.len(), 1);
    assert_eq!(
        snapshot.packages[0]
            .candidate_version
            .as_ref()
            .map(|version| version.as_str()),
        Some("8.6.0")
    );
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn execution_support_persists_redacted_logs_summaries_reconciliation_and_inventory() {
    let Some((database, admin, schema)) = isolated_repositories().await else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured or unavailable");
        return;
    };
    let repos = Repositories::new(&database);
    let target = Target::new(
        "execution-support-test",
        TargetKind::LinuxServer,
        "192.0.2.30",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repos.targets.save(&target).await.unwrap();

    let mut applying = package_update_job(&target, &format!("apply-{}", Uuid::new_v4()));
    applying.transition_to(AnsibleJobStatus::Planned).unwrap();
    repos.ansible_jobs.save(&applying).await.unwrap();
    execution_support::mark_job_applying(&repos, &mut applying)
        .await
        .unwrap();
    assert_eq!(
        execution_support::mark_job_applying(&repos, &mut applying).await,
        Err(JobFailureCode::PlaybookFailed)
    );
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(applying.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Applying
    );
    assert!(
        repos
            .ansible_jobs
            .events(applying.id)
            .await
            .unwrap()
            .iter()
            .any(|event| matches!(
                event.event,
                JobEventKind::StatusChanged {
                    status: AnsibleJobStatus::Applying
                }
            ))
    );

    let context = InvocationContext {
        inventory: PathBuf::new(),
        vars_file: PathBuf::new(),
        credential: SecretValue::new("ssh-password").unwrap(),
        agent_token: Some(SecretValue::new("agent-token").unwrap()),
    };
    let streamed = worker_output(true, "connected with ssh-password", "token agent-token\n");
    execution_support::emit_worker_streams(&repos, applying.id, &streamed, &context)
        .await
        .unwrap();
    let events = repos.ansible_jobs.events(applying.id).await.unwrap();
    let logs = events
        .iter()
        .filter_map(|event| match &event.event {
            JobEventKind::WorkerLog { source, message } => {
                Some((source.as_str(), message.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0], ("stdout", "connected with [REDACTED]"));
    assert_eq!(logs[1], ("stderr", "token [REDACTED]\n"));
    assert!(!format!("{events:?}").contains("ssh-password"));
    assert!(!format!("{events:?}").contains("agent-token"));
    execution_support::emit_worker_streams(
        &repos,
        applying.id,
        &worker_output(true, "  \n", ""),
        &context,
    )
    .await
    .unwrap();
    assert_eq!(
        repos.ansible_jobs.events(applying.id).await.unwrap().len(),
        events.len()
    );

    let apply_output = worker_output(
        true,
        "TASK [Install package]\nchanged: [target]\nTASK [Verify package]\nok: [target]\nPLAY RECAP\ntarget : ok=2 changed=1 failed=0",
        "",
    );
    execution_support::emit_mode_summary(&repos, &applying, &apply_output)
        .await
        .unwrap();
    let events = repos.ansible_jobs.events(applying.id).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        JobEventKind::TaskFinished { task, changed: true } if task == "Install package"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event,
        JobEventKind::WorkerLog { source, message } if source == "apply" && message.contains("1 Task(s)")
    )));
    applying.transition_to(AnsibleJobStatus::Succeeded).unwrap();
    repos.ansible_jobs.update(&applying).await.unwrap();

    for (mode, source_name) in [
        (lxcup_ansible::ExecutionMode::Check, "check"),
        (lxcup_ansible::ExecutionMode::Plan, "plan"),
    ] {
        let mut summary_job = health_job(&target, &format!("summary-{}", Uuid::new_v4()));
        summary_job.mode = mode;
        repos.ansible_jobs.save(&summary_job).await.unwrap();
        execution_support::emit_mode_summary(&repos, &summary_job, &apply_output)
            .await
            .unwrap();
        assert!(
            repos
                .ansible_jobs
                .events(summary_job.id)
                .await
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.event,
                    JobEventKind::WorkerLog { source, .. } if source == source_name
                ))
        );
    }

    let mut reconcile_noop = health_job(&target, &format!("reconcile-noop-{}", Uuid::new_v4()));
    reconcile_noop.mode = lxcup_ansible::ExecutionMode::Reconcile;
    repos.ansible_jobs.save(&reconcile_noop).await.unwrap();
    execution_support::emit_mode_summary(&repos, &reconcile_noop, &apply_output)
        .await
        .unwrap();
    assert!(
        repos
            .ansible_jobs
            .events(reconcile_noop.id)
            .await
            .unwrap()
            .is_empty()
    );

    let mut source = package_update_job(&target, &format!("source-{}", Uuid::new_v4()));
    source.transition_to(AnsibleJobStatus::Planned).unwrap();
    source.transition_to(AnsibleJobStatus::Applying).unwrap();
    source
        .transition_to(AnsibleJobStatus::ReconcileRequired)
        .unwrap();
    repos.ansible_jobs.save(&source).await.unwrap();
    let failed_reconcile = worker_output(false, "", "connection refused");
    execution_support::reconcile_job(&repos, reconcile_noop.id, source.clone(), &failed_reconcile)
        .await
        .unwrap();
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(source.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::ReconcileRequired
    );
    let completed_reconcile = worker_output(
        true,
        "PLAY RECAP\ntarget : ok=2 changed=0 failed=0 skipped=0",
        "",
    );
    execution_support::reconcile_job(
        &repos,
        reconcile_noop.id,
        source.clone(),
        &completed_reconcile,
    )
    .await
    .unwrap();
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(source.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Succeeded
    );
    let mut source_with_changes =
        package_update_job(&target, &format!("source-changes-{}", Uuid::new_v4()));
    source_with_changes
        .transition_to(AnsibleJobStatus::Planned)
        .unwrap();
    source_with_changes
        .transition_to(AnsibleJobStatus::Applying)
        .unwrap();
    source_with_changes
        .transition_to(AnsibleJobStatus::ReconcileRequired)
        .unwrap();
    repos.ansible_jobs.save(&source_with_changes).await.unwrap();
    let changed_reconcile = worker_output(
        true,
        "PLAY RECAP\ntarget : ok=2 changed=1 failed=0 skipped=0",
        "",
    );
    execution_support::reconcile_job(
        &repos,
        reconcile_noop.id,
        source_with_changes.clone(),
        &changed_reconcile,
    )
    .await
    .unwrap();
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(source_with_changes.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Failed
    );

    let inventory_dir = std::env::temp_dir().join(format!("lxcup-exec-support-{}", Uuid::new_v4()));
    fs::create_dir_all(&inventory_dir).unwrap();
    assert_eq!(
        execution_support::persist_package_inventory(
            &repos,
            applying.id,
            &inventory_dir,
            target.id,
        )
        .await,
        Err(JobFailureCode::PlaybookFailed)
    );
    fs::write(
        inventory_dir.join("package-inventory.json"),
        r#"{"packages":{"curl":[{"version":"8.5.0","arch":"amd64"}]},"upgradable":["curl/stable 8.6.0 amd64 [upgradable from: 8.5.0]"]}"#,
    )
    .unwrap();
    execution_support::persist_package_inventory(&repos, applying.id, &inventory_dir, target.id)
        .await
        .unwrap();
    let inventory = repos
        .package_inventory
        .find_latest(target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inventory.snapshot.packages.len(), 1);
    assert!(repos
        .ansible_jobs
        .events(applying.id)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(
            &event.event,
            JobEventKind::TaskFinished { task, changed: false } if task == "package inventory: 1 packages"
        )));

    fs::remove_dir_all(inventory_dir).unwrap();
    drop_test_schema(database, admin, &schema).await;
}

#[tokio::test]
async fn artifact_download_checks_version_platform_and_sha256() {
    let root = std::env::temp_dir().join(format!("lxcup-worker-artifact-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let binary = b"verified test agent".to_vec();
    let digest = format!("{:x}", Sha256::digest(&binary));
    let base_manifest = serde_json::json!({
        "version": "0.3.1",
        "artifacts": [{"platform": "linux-amd64", "file": "agent", "sha256": digest}]
    });

    let (base, server) = artifact_server(base_manifest.clone(), binary.clone(), 2).await;
    let mut runtime = runtime_with_secrets(&root).0;
    runtime.artifacts = base;
    let path = artifact(&runtime, "0.3.1", &root).await.unwrap();
    assert_eq!(fs::read(path).unwrap(), binary);
    server.await.unwrap();

    let mut wrong_version = base_manifest.clone();
    wrong_version["version"] = serde_json::json!("0.1.0");
    let (base, server) = artifact_server(wrong_version, Vec::new(), 1).await;
    runtime.artifacts = base;
    assert_eq!(
        artifact(&runtime, "0.3.1", &root).await,
        Err(JobFailureCode::PlaybookFailed)
    );
    server.await.unwrap();

    let mut wrong_hash = base_manifest;
    wrong_hash["artifacts"][0]["sha256"] = serde_json::json!("00");
    let (base, server) = artifact_server(wrong_hash, binary, 2).await;
    runtime.artifacts = base;
    assert_eq!(
        artifact(&runtime, "0.3.1", &root).await,
        Err(JobFailureCode::PlaybookFailed)
    );
    server.await.unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn recovery_and_queue_processing_persist_all_worker_outcomes() {
    let Some((database, admin, schema)) = isolated_repositories().await else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured or unavailable");
        return;
    };
    let repos = Repositories::new(&database);
    let credential_ref = SecretId::new();
    let target = Target::new(
        "worker-flow-test",
        TargetKind::LinuxServer,
        "192.0.2.20",
        TargetTransport::Ssh,
        credential_ref,
        SecretId::new(),
    )
    .unwrap();
    repos.targets.save(&target).await.unwrap();

    let queued = health_job(&target, &format!("queued-{}", Uuid::new_v4()));
    repos.ansible_jobs.save(&queued).await.unwrap();
    let root = std::env::temp_dir().join(format!("lxcup-worker-flow-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let (runtime, _, _, _) = runtime_with_secrets(&root);
    process(&repos, &runtime, "test-worker").await.unwrap();
    let failed = repos
        .ansible_jobs
        .find_by_id(queued.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AnsibleJobStatus::Failed);
    assert!(matches!(
        repos
            .ansible_jobs
            .events(queued.id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .event,
        JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Failed
        }
    ));

    let mut non_target_job = health_job(&target, &format!("non-target-{}", Uuid::new_v4()));
    non_target_job.target = ResourceTarget::Container(lxcup_core::ContainerId::new(7));
    assert_eq!(
        run(&repos, &runtime, &mut non_target_job).await,
        Err(JobFailureCode::PlaybookFailed)
    );

    let mut checking = health_job(&target, &format!("checking-{}", Uuid::new_v4()));
    checking.transition_to(AnsibleJobStatus::Checking).unwrap();
    repos.ansible_jobs.save(&checking).await.unwrap();
    let planned_target = Target::new(
        "worker-planned-test",
        TargetKind::LinuxServer,
        "192.0.2.21",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repos.targets.save(&planned_target).await.unwrap();
    let mut planned = health_job(&planned_target, &format!("planned-{}", Uuid::new_v4()));
    planned.transition_to(AnsibleJobStatus::Planned).unwrap();
    repos.ansible_jobs.save(&planned).await.unwrap();
    let applying_target = Target::new(
        "worker-applying-test",
        TargetKind::LinuxServer,
        "192.0.2.22",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    repos.targets.save(&applying_target).await.unwrap();
    let mut applying = health_job(&applying_target, &format!("applying-{}", Uuid::new_v4()));
    applying.transition_to(AnsibleJobStatus::Planned).unwrap();
    applying.transition_to(AnsibleJobStatus::Applying).unwrap();
    repos.ansible_jobs.save(&applying).await.unwrap();
    recover_interrupted_jobs(&repos).await.unwrap();
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(checking.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Failed
    );
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(planned.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::Failed
    );
    assert_eq!(
        repos
            .ansible_jobs
            .find_by_id(applying.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AnsibleJobStatus::ReconcileRequired
    );

    fs::remove_dir_all(root).unwrap();
    drop_test_schema(database, admin, &schema).await;
}
