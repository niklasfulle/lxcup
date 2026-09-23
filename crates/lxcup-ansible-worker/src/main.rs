use chrono::Utc;
use lxcup_ansible::{
    AnsibleJob, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, JobEvent, JobEventKind,
    JobFailureCode,
};
use lxcup_core::{
    ResourceTarget, SecretKind, SecretValue, Target, TargetKind, TargetTransport,
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
    let worker_name = std::env::var("LXCUP_WORKER_NAME")
        .unwrap_or_else(|_| "lxcup-ansible-worker".to_owned());
    recover_interrupted_jobs(&repos)
        .await
        .expect("worker recovery");
    loop {
        if let Err(error) = repos.worker_heartbeats.record(&worker_name).await {
            tracing::error!(?error, "worker heartbeat failed");
        }
        if let Err(e) = process(&repos, &runtime).await {
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
        let root = std::env::var("LXCUP_SECRET_STORE_DIR")
            .map_err(|_| "LXCUP_SECRET_STORE_DIR is missing")?;
        let key = SecretMasterKey::from_env("LXCUP_SECRET_MASTER_KEY")
            .map_err(|_| "LXCUP_SECRET_MASTER_KEY must be 64 hex characters")?;
        Ok(Self {
            secrets: EncryptedFileSecretStore::new(root, key, [])
                .map_err(|_| "secret store directory is unavailable")?,
            artifacts: std::env::var("LXCUP_ARTIFACT_BASE_URL")
                .map_err(|_| "LXCUP_ARTIFACT_BASE_URL is missing")?
                .trim_end_matches('/')
                .into(),
            user: std::env::var("LXCUP_WORKER_SSH_USER").unwrap_or_else(|_| "lxcup".into()),
            controller_url: std::env::var("LXCUP_CONTROLLER_URL")
                .ok()
                .map(|value| value.trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty()),
        })
    }
}
async fn process(
    repos: &Repositories,
    runtime: &Runtime,
) -> Result<(), lxcup_persistence::RepositoryError> {
    while let Some(mut job) = repos.ansible_jobs.claim_next_queued().await? {
        let result = run(repos, runtime, &mut job).await;
        let (status, job_event) = match result {
            Ok(changed) => (
                AnsibleJobStatus::Succeeded,
                JobEventKind::TaskFinished {
                    task: job.playbook.clone(),
                    changed,
                },
            ),
            Err(code) => (AnsibleJobStatus::Failed, JobEventKind::Failed { code }),
        };
        job.transition_to(status).expect("claimed status");
        repos.ansible_jobs.update(&job).await?;
        event(repos, job.id, job_event).await?;
        event(repos, job.id, JobEventKind::StatusChanged { status }).await?;
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
    let context = prepare_invocation(r, job, target, dir).await?;
    job.transition_to(AnsibleJobStatus::Applying)
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
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
    if !out.status.success() {
        let output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return Err(classify_playbook_failure(&output));
    };
    Ok(playbook_changed(&String::from_utf8_lossy(&out.stdout)))
}

fn classify_playbook_failure(output: &str) -> JobFailureCode {
    if output.contains("Invalid/incorrect password")
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
        _ => None,
    }
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
mod tests {
    use super::{classify_playbook_failure, playbook_changed, *};
    use lxcup_ansible::{AnsibleJobRequest, ExecutionMode};
    use lxcup_core::{ActorRole, ResourceLifecycle, SecretId, SecretScope};
    use lxcup_secrets::CreateSecret;

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
            classify_playbook_failure("role was not found"),
            JobFailureCode::PlaybookFailed
        );
        assert!(!playbook_changed("PLAY RECAP changed=0 failed=0"));
        assert!(playbook_changed("PLAY RECAP changed=1 failed=0"));
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
            mode: ExecutionMode::Check,
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
}
