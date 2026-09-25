use chrono::Utc;
use lxcup_ansible::{
    AnsibleJob, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, JobEvent, JobEventKind,
    JobFailureCode,
};
use lxcup_core::{
    InstalledPackage, PackageInventorySnapshot, PackageName, PackageVersion, ResourceTarget,
    SecretKind, SecretValue, Target, TargetKind, TargetTransport,
};
use lxcup_persistence::{Database, Repositories};
use lxcup_secrets::{EncryptedFileSecretStore, SecretMasterKey, SecretStore};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};
use tracing::Instrument;
use uuid::Uuid;

#[derive(Deserialize)]
struct Manifest {
    version: String,
    artifacts: Vec<Artifact>,
}
#[derive(Deserialize)]
struct Artifact {
    platform: String,
    file: String,
    sha256: String,
}
struct Runtime {
    secrets: EncryptedFileSecretStore,
    artifacts: String,
    user: String,
    controller_url: Option<String>,
}

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-ansible-worker");
    let db = Database::connect_from_env().await.expect("DATABASE_URL");
    db.migrate().await.expect("migrations");
    let repos = Repositories::new(&db);
    let runtime =
        Runtime::load().unwrap_or_else(|error| panic!("worker configuration invalid: {error}"));
    let worker_name =
        std::env::var("LXCUP_WORKER_NAME").unwrap_or_else(|_| "lxcup-ansible-worker".to_owned());
    recover_interrupted_jobs(&repos)
        .await
        .expect("worker recovery");
    loop {
        if let Err(error) = repos.worker_heartbeats.record(&worker_name).await {
            tracing::error!(?error, "worker heartbeat failed");
        }
        if let Err(e) = process(&repos, &runtime, &worker_name).await {
            tracing::error!(?e, "worker cycle failed");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn recover_interrupted_jobs(
    repos: &Repositories,
) -> Result<(), lxcup_persistence::RepositoryError> {
    for mut job in repos.ansible_jobs.list().await? {
        if !matches!(
            job.status,
            AnsibleJobStatus::Checking | AnsibleJobStatus::Planned | AnsibleJobStatus::Applying
        ) {
            continue;
        }
        if job.status == AnsibleJobStatus::Applying {
            job.transition_to(AnsibleJobStatus::ReconcileRequired)
                .expect("applying jobs require reconciliation");
            repos.ansible_jobs.update(&job).await?;
            event(repos, job.id, JobEventKind::ReconcileRequired).await?;
            event(
                repos,
                job.id,
                JobEventKind::StatusChanged {
                    status: AnsibleJobStatus::ReconcileRequired,
                },
            )
            .await?;
        } else {
            job.transition_to(AnsibleJobStatus::Failed)
                .expect("interrupted pre-apply jobs can fail");
            repos.ansible_jobs.update(&job).await?;
            event(
                repos,
                job.id,
                JobEventKind::Failed {
                    code: JobFailureCode::WorkerRestarted,
                },
            )
            .await?;
            event(
                repos,
                job.id,
                JobEventKind::StatusChanged {
                    status: AnsibleJobStatus::Failed,
                },
            )
            .await?;
        }
    }
    Ok(())
}
impl Runtime {
    fn load() -> Result<Self, &'static str> {
        Self::from_values(
            std::env::var("LXCUP_SECRET_STORE_DIR").ok(),
            SecretMasterKey::from_env("LXCUP_SECRET_MASTER_KEY").ok(),
            std::env::var("LXCUP_ARTIFACT_BASE_URL").ok(),
            std::env::var("LXCUP_WORKER_SSH_USER").ok(),
            std::env::var("LXCUP_CONTROLLER_URL").ok(),
        )
    }

    fn from_values(
        root: Option<String>,
        key: Option<SecretMasterKey>,
        artifacts: Option<String>,
        user: Option<String>,
        controller_url: Option<String>,
    ) -> Result<Self, &'static str> {
        let root = root.ok_or("LXCUP_SECRET_STORE_DIR is missing")?;
        let key = key.ok_or("LXCUP_SECRET_MASTER_KEY must be 64 hex characters")?;
        Ok(Self {
            secrets: EncryptedFileSecretStore::new(root, key, [])
                .map_err(|_| "secret store directory is unavailable")?,
            artifacts: artifacts
                .ok_or("LXCUP_ARTIFACT_BASE_URL is missing")?
                .trim_end_matches('/')
                .to_owned(),
            user: user.unwrap_or_else(|| "lxcup".into()),
            controller_url: controller_url
                .map(|value| value.trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty()),
        })
    }
}
async fn process(
    repos: &Repositories,
    runtime: &Runtime,
    worker_name: &str,
) -> Result<(), lxcup_persistence::RepositoryError> {
    while let Some(mut job) = repos.ansible_jobs.claim_next_queued().await? {
        let span = tracing::info_span!(
            "worker_job",
            worker_id = %worker_name,
            job_id = ?job.id,
            target = ?job.target
        );
        async {
            tracing::info!(status = ?job.status, "claimed workflow job");
            let result = run(repos, runtime, &mut job).await;
            let (status, job_event) = match result {
                Ok(changed) => {
                    tracing::info!(changed, "workflow job execution completed");
                    (
                        AnsibleJobStatus::Succeeded,
                        JobEventKind::TaskFinished {
                            task: job.playbook.clone(),
                            changed,
                        },
                    )
                }
                Err(code) => {
                    tracing::error!(failure_code = ?code, "workflow job execution failed");
                    (AnsibleJobStatus::Failed, JobEventKind::Failed { code })
                }
            };
            job.transition_to(status).expect("claimed status");
            repos.ansible_jobs.update(&job).await?;
            event(repos, job.id, job_event).await?;
            event(repos, job.id, JobEventKind::StatusChanged { status }).await?;
            tracing::info!(status = ?status, "workflow job status persisted");
            Ok::<(), lxcup_persistence::RepositoryError>(())
        }
        .instrument(span)
        .await?;
    }
    Ok(())
}
async fn run(
    repos: &Repositories,
    r: &Runtime,
    job: &mut AnsibleJob,
) -> Result<bool, JobFailureCode> {
    let ResourceTarget::Target(id) = job.target else {
        return Err(JobFailureCode::PlaybookFailed);
    };
    let target = repos
        .targets
        .find_by_id(id)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .ok_or(JobFailureCode::Unreachable)?;
    if job
        .secret_refs
        .iter()
        .any(|s| *s != target.credential_secret_ref && *s != target.agent_secret_ref)
    {
        return Err(JobFailureCode::InvalidCredentials);
    };
    job.transition_to(AnsibleJobStatus::Planned)
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    repos
        .ansible_jobs
        .update(job)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(
        repos,
        job.id,
        JobEventKind::StatusChanged {
            status: AnsibleJobStatus::Planned,
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    let dir = std::env::temp_dir().join(format!("lxcup-{}-{}", job.id.as_uuid(), Uuid::new_v4()));
    fs::create_dir_all(&dir).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    let result = invoke(repos, r, job, &target, &dir).await;
    let _ = fs::remove_dir_all(dir);
    result
}
async fn invoke(
    repos: &Repositories,
    r: &Runtime,
    job: &mut AnsibleJob,
    target: &Target,
    dir: &Path,
) -> Result<bool, JobFailureCode> {
    let mode_args = ansible_mode_args(job.operation, job.mode)?;
    let context = prepare_invocation(r, job, target, dir).await?;
    if job.mode == lxcup_ansible::ExecutionMode::Apply {
        job.transition_to(AnsibleJobStatus::Applying)
            .map_err(|_| JobFailureCode::PlaybookFailed)?;
    }
    let playbook = playbook(job.operation, target.kind).ok_or(JobFailureCode::PlaybookFailed)?;
    let out = timeout(
        Duration::from_secs(900),
        Command::new("ansible-playbook")
            .env("ANSIBLE_ROLES_PATH", "/opt/lxcup/ansible/roles")
            .current_dir("/opt/lxcup/ansible")
            .args([
                "-i",
                context
                    .inventory
                    .to_str()
                    .ok_or(JobFailureCode::WorkerUnavailable)?,
                playbook,
                "--extra-vars",
                &format!("@{}", context.vars_file.display()),
            ])
            .args(mode_args)
            .output(),
    )
    .await
    .map_err(|_| JobFailureCode::Timeout)?
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    let redactions = [
        Some(context.credential.expose()),
        context.agent_token.as_ref().map(|value| value.expose()),
    ];
    for (source, output) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        let message = redact_output(&String::from_utf8_lossy(output), &redactions);
        if !message.trim().is_empty() {
            event(
                repos,
                job.id,
                JobEventKind::WorkerLog {
                    source: source.to_owned(),
                    message,
                },
            )
            .await
            .map_err(|_| JobFailureCode::WorkerUnavailable)?;
        }
    }
    if job.mode == lxcup_ansible::ExecutionMode::Check {
        event(
            repos,
            job.id,
            JobEventKind::WorkerLog {
                source: "check".to_owned(),
                message: check_mode_summary(&String::from_utf8_lossy(&out.stdout)),
            },
        )
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    }
    if !out.status.success() {
        let output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return Err(classify_playbook_failure(&output));
    };
    if job.operation == AnsibleOperation::CollectPackageInventory {
        let snapshot = read_package_inventory(dir, target.id)?;
        repos
            .package_inventory
            .replace(&snapshot)
            .await
            .map_err(|_| JobFailureCode::WorkerUnavailable)?;
        event(
            repos,
            job.id,
            JobEventKind::TaskFinished {
                task: format!("package inventory: {} packages", snapshot.packages.len()),
                changed: false,
            },
        )
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    }
    Ok(job.mode == lxcup_ansible::ExecutionMode::Apply
        && playbook_changed(&String::from_utf8_lossy(&out.stdout)))
}

fn ansible_mode_args(
    operation: AnsibleOperation,
    mode: lxcup_ansible::ExecutionMode,
) -> Result<&'static [&'static str], JobFailureCode> {
    if !operation.supports_mode(mode) {
        return Err(JobFailureCode::PlaybookFailed);
    }
    match mode {
        lxcup_ansible::ExecutionMode::Check => Ok(
            if matches!(
                operation,
                AnsibleOperation::HealthCheck | AnsibleOperation::CollectPackageInventory
            ) {
                &[]
            } else {
                &["--check"]
            },
        ),
        lxcup_ansible::ExecutionMode::Plan | lxcup_ansible::ExecutionMode::Apply => Ok(&[]),
        lxcup_ansible::ExecutionMode::Reconcile => Err(JobFailureCode::PlaybookFailed),
    }
}

fn check_mode_summary(stdout: &str) -> String {
    if stdout
        .lines()
        .any(|line| line.trim_start().starts_with("skipping:"))
    {
        "Prüfung abgeschlossen, aber mindestens ein Ansible-Schritt wurde übersprungen und ist nicht prüfbar. Details stehen in der Worker-Ausgabe.".to_owned()
    } else if !stdout.contains("PLAY RECAP") {
        "Prüfung lieferte keine vollständige Ansible-Zusammenfassung; Ergebnis bitte manuell prüfen.".to_owned()
    } else if playbook_changed(stdout) {
        "Prüfung erfolgreich: Ansible hat mögliche Änderungen erkannt, aber im Check-Modus nichts angewendet.".to_owned()
    } else {
        "Prüfung erfolgreich: Ansible hat keine Änderungen erkannt und nichts angewendet."
            .to_owned()
    }
}

fn classify_playbook_failure(output: &str) -> JobFailureCode {
    if output.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || output.contains("Host key verification failed")
        || output.contains("host key for")
    {
        JobFailureCode::HostKeyChanged
    } else if output.contains("Invalid/incorrect password")
        || output.contains("Permission denied, please try again")
    {
        JobFailureCode::InvalidCredentials
    } else if output.contains("UNREACHABLE!") || output.contains("unreachable: true") {
        JobFailureCode::Unreachable
    } else {
        JobFailureCode::PlaybookFailed
    }
}

fn playbook_changed(stdout: &str) -> bool {
    !stdout.contains("changed=0")
}

struct InvocationContext {
    inventory: PathBuf,
    vars_file: PathBuf,
    credential: SecretValue,
    agent_token: Option<SecretValue>,
}

async fn prepare_invocation(
    r: &Runtime,
    job: &AnsibleJob,
    target: &Target,
    dir: &Path,
) -> Result<InvocationContext, JobFailureCode> {
    let credential = r
        .secrets
        .read(target.credential_secret_ref)
        .map_err(|_| JobFailureCode::InvalidCredentials)?;
    let kind = r
        .secrets
        .metadata(target.credential_secret_ref)
        .map_err(|_| JobFailureCode::InvalidCredentials)?
        .metadata
        .kind;
    let mut host = serde_json::json!({
        "ansible_host": target.address,
        "ansible_user": target.ssh_user.as_deref().unwrap_or(&r.user),
        "ansible_remote_tmp": "/tmp/.ansible/tmp"
    });
    let key = dir.join("credential");
    let known_hosts = prepare_known_hosts(r, target, dir)?;
    match (target.transport, kind) {
        (TargetTransport::Ssh, SecretKind::SshPrivateKey) => {
            private(&key, credential.expose()).map_err(|_| JobFailureCode::WorkerUnavailable)?;
            host["ansible_ssh_private_key_file"] = serde_json::json!(key);
        }
        (TargetTransport::Ssh, SecretKind::SshPassword) => {
            host["ansible_password"] = serde_json::json!(credential.expose());
            host["ansible_become_password"] = serde_json::json!(credential.expose());
        }
        _ => return Err(JobFailureCode::InvalidCredentials),
    }
    if let Some(path) = known_hosts {
        host["ansible_ssh_common_args"] =
            serde_json::json!(format!("-o UserKnownHostsFile={}", path.display()));
    }
    let group = if target.kind == TargetKind::WindowsServer {
        "lxcup_windows_targets"
    } else {
        "lxcup_targets"
    };
    let inventory = dir.join("inventory.json");
    private(
        &inventory,
        &serde_json::json!({group:{"hosts":{"target":host}}}).to_string(),
    )
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    let mut vars =
        serde_json::json!({"lxcup_execution_mode":format!("{:?}",job.mode).to_lowercase()});
    let agent_token = prepare_agent_vars(r, job, target, &mut vars)?;
    if let Some(controller_url) = &r.controller_url {
        vars["lxcup_controller_url"] = serde_json::json!(controller_url);
    }
    if let AnsibleParameters::DeployAgent { agent_version }
    | AnsibleParameters::UpdateAgent { agent_version } = &job.parameters
    {
        vars["lxcup_agent_version"] = serde_json::json!(agent_version);
        vars["lxcup_agent_binary_src"] = serde_json::json!(artifact(r, agent_version, dir).await?);
    }
    if let AnsibleParameters::UpdatePackages { packages } = &job.parameters {
        vars["lxcup_update_packages"] = serde_json::json!(packages);
        vars["lxcup_update_request_hash"] = serde_json::json!(job.parameter_hash);
        vars["lxcup_plan_confirmed"] =
            // The server contract only creates mutating Apply jobs after an explicit confirmation.
            serde_json::json!(job.mode == lxcup_ansible::ExecutionMode::Apply);
        vars["lxcup_distribution_upgrade"] = serde_json::json!(false);
    }
    if job.operation == AnsibleOperation::CollectPackageInventory {
        vars["lxcup_package_inventory_output"] =
            serde_json::json!(dir.join("package-inventory.json"));
        vars["lxcup_package_inventory_remote_file"] =
            serde_json::json!(if target.kind == TargetKind::WindowsServer {
                "C:\\Windows\\Temp\\lxcup-package-inventory.json"
            } else {
                "/tmp/lxcup-package-inventory.json"
            });
    }
    let vars_file = dir.join("vars.json");
    private(&vars_file, &vars.to_string()).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(InvocationContext {
        inventory,
        vars_file,
        credential,
        agent_token,
    })
}

fn prepare_known_hosts(
    r: &Runtime,
    target: &Target,
    dir: &Path,
) -> Result<Option<PathBuf>, JobFailureCode> {
    if target.transport != TargetTransport::Ssh {
        return Ok(None);
    }
    let secret_ref = target
        .ssh_known_hosts_secret_ref
        .ok_or(JobFailureCode::InvalidCredentials)?;
    let value = r
        .secrets
        .read(secret_ref)
        .map_err(|_| JobFailureCode::InvalidCredentials)?;
    let metadata = r
        .secrets
        .metadata(secret_ref)
        .map_err(|_| JobFailureCode::InvalidCredentials)?;
    if metadata.metadata.kind != SecretKind::SshKnownHosts {
        return Err(JobFailureCode::InvalidCredentials);
    }
    let path = dir.join("known_hosts");
    private(&path, value.expose()).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(Some(path))
}

fn prepare_agent_vars(
    r: &Runtime,
    job: &AnsibleJob,
    target: &Target,
    vars: &mut serde_json::Value,
) -> Result<Option<SecretValue>, JobFailureCode> {
    if matches!(
        job.operation,
        AnsibleOperation::HealthCheck | AnsibleOperation::ConfigureTarget
    ) {
        return Ok(None);
    }
    let token = r
        .secrets
        .read(target.agent_secret_ref)
        .map_err(|_| JobFailureCode::InvalidCredentials)?;
    vars["lxcup_agent_token"] = serde_json::json!(token.expose());
    vars["lxcup_agent_id"] = serde_json::json!(target.id.as_uuid().to_string());
    Ok(Some(token))
}

fn redact_output(output: &str, secrets: &[Option<&str>]) -> String {
    secrets
        .iter()
        .flatten()
        .fold(output.to_owned(), |value, secret| {
            if secret.is_empty() {
                value
            } else {
                value.replace(secret, "[REDACTED]")
            }
        })
}
async fn artifact(r: &Runtime, version: &str, dir: &Path) -> Result<String, JobFailureCode> {
    let base = format!("{}/agent/{version}", r.artifacts);
    let m: Manifest = reqwest::get(format!("{base}/manifest.json"))
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .error_for_status()
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .json()
        .await
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    if m.version != version {
        return Err(JobFailureCode::PlaybookFailed);
    }
    let a = m
        .artifacts
        .into_iter()
        .find(|a| a.platform == "linux-amd64")
        .ok_or(JobFailureCode::PlaybookFailed)?;
    let b = reqwest::get(format!("{base}/{}", a.file))
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .bytes()
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    if format!("{:x}", Sha256::digest(&b)) != a.sha256 {
        return Err(JobFailureCode::PlaybookFailed);
    }
    let p = dir.join("agent");
    fs::write(&p, b).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(p.display().to_string())
}
fn playbook(o: AnsibleOperation, k: TargetKind) -> Option<&'static str> {
    match (o, k) {
        (
            AnsibleOperation::DeployAgent | AnsibleOperation::UpdateAgent,
            TargetKind::WindowsServer,
        ) => Some("playbooks/agent-windows.yml"),
        (AnsibleOperation::DeployAgent | AnsibleOperation::UpdateAgent, _) => {
            Some("playbooks/agent-linux.yml")
        }
        (AnsibleOperation::RepairAgent, _) => Some("playbooks/agent-linux-repair.yml"),
        (AnsibleOperation::UpdatePackages, TargetKind::WindowsServer) => {
            Some("playbooks/packages-windows.yml")
        }
        (AnsibleOperation::UpdatePackages, _) => Some("playbooks/packages-linux.yml"),
        (AnsibleOperation::HealthCheck, TargetKind::WindowsServer) => {
            Some("playbooks/health-check-windows.yml")
        }
        (AnsibleOperation::HealthCheck, _) => Some("playbooks/health-check-linux.yml"),
        (AnsibleOperation::CollectPackageInventory, TargetKind::WindowsServer) => {
            Some("playbooks/package-inventory-windows.yml")
        }
        (AnsibleOperation::CollectPackageInventory, _) => {
            Some("playbooks/package-inventory-linux.yml")
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct CollectedPackageInventory {
    packages: serde_json::Value,
}

fn read_package_inventory(
    dir: &Path,
    target_id: lxcup_core::TargetId,
) -> Result<PackageInventorySnapshot, JobFailureCode> {
    let contents = fs::read_to_string(dir.join("package-inventory.json"))
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    let collected: CollectedPackageInventory =
        serde_json::from_str(&contents).map_err(|_| JobFailureCode::PlaybookFailed)?;
    let packages = normalize_package_inventory(collected.packages)?;
    Ok(PackageInventorySnapshot {
        target_id,
        collected_at: Utc::now(),
        packages,
    })
}

fn normalize_package_inventory(
    value: serde_json::Value,
) -> Result<Vec<InstalledPackage>, JobFailureCode> {
    let mut packages = Vec::new();
    let Some(by_name) = value.as_object() else {
        return Err(JobFailureCode::PlaybookFailed);
    };
    for (name, entries) in by_name {
        let entries = entries.as_array().ok_or(JobFailureCode::PlaybookFailed)?;
        for entry in entries {
            let version = entry
                .get("version")
                .and_then(serde_json::Value::as_str)
                .ok_or(JobFailureCode::PlaybookFailed)?;
            packages.push(InstalledPackage {
                name: PackageName::new(name.clone()).map_err(|_| JobFailureCode::PlaybookFailed)?,
                version: PackageVersion::new(version.to_owned())
                    .map_err(|_| JobFailureCode::PlaybookFailed)?,
                architecture: entry
                    .get("arch")
                    .or_else(|| entry.get("architecture"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                source: entry
                    .get("source")
                    .or_else(|| entry.get("provider"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    if packages.len() > 50_000 {
        return Err(JobFailureCode::PlaybookFailed);
    }
    packages.sort_by(|left, right| {
        left.name
            .as_str()
            .cmp(right.name.as_str())
            .then_with(|| left.version.as_str().cmp(right.version.as_str()))
    });
    Ok(packages)
}
fn private(p: &Path, s: &str) -> std::io::Result<()> {
    fs::write(p, s)
}
async fn event(
    r: &Repositories,
    id: lxcup_core::AnsibleJobId,
    e: JobEventKind,
) -> Result<(), lxcup_persistence::RepositoryError> {
    let n = r.ansible_jobs.events(id).await?.len() as u64 + 1;
    r.ansible_jobs
        .append_event(&JobEvent {
            sequence: n,
            job_id: id,
            event: e,
            created_at: Utc::now(),
        })
        .await
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
