use chrono::Utc;
use lxcup_ansible::{
    AnsibleJob, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, JobEvent, JobEventKind,
    JobFailureCode,
};
use lxcup_core::{ResourceTarget, SecretKind, Target, TargetKind, TargetTransport};
use lxcup_persistence::{Database, Repositories};
use lxcup_secrets::{EncryptedFileSecretStore, SecretMasterKey, SecretStore};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Duration};
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
}

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-ansible-worker");
    let db = Database::connect_from_env().await.expect("DATABASE_URL");
    db.migrate().await.expect("migrations");
    let repos = Repositories::new(&db);
    let runtime =
        Runtime::load().unwrap_or_else(|error| panic!("worker configuration invalid: {error}"));
    recover_interrupted_jobs(&repos)
        .await
        .expect("worker recovery");
    loop {
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
    let result = invoke(r, job, &target, &dir).await;
    let _ = fs::remove_dir_all(dir);
    result
}
async fn invoke(
    r: &Runtime,
    job: &mut AnsibleJob,
    target: &Target,
    dir: &Path,
) -> Result<bool, JobFailureCode> {
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
    let mut host = serde_json::json!({"ansible_host":target.address,"ansible_user":r.user});
    let key = dir.join("credential");
    match (target.transport, kind) {
        (TargetTransport::Ssh, SecretKind::SshPrivateKey) => {
            private(&key, credential.expose()).map_err(|_| JobFailureCode::WorkerUnavailable)?;
            host["ansible_ssh_private_key_file"] = serde_json::json!(key)
        }
        (TargetTransport::Ssh, SecretKind::SshPassword) => {
            host["ansible_password"] = serde_json::json!(credential.expose())
        }
        _ => return Err(JobFailureCode::InvalidCredentials),
    };
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
    if !matches!(
        job.operation,
        AnsibleOperation::HealthCheck | AnsibleOperation::ConfigureTarget
    ) {
        vars["lxcup_agent_token"] = serde_json::json!(
            r.secrets
                .read(target.agent_secret_ref)
                .map_err(|_| JobFailureCode::InvalidCredentials)?
                .expose()
        );
        vars["lxcup_agent_id"] = serde_json::json!(target.id.as_uuid().to_string())
    };
    if let AnsibleParameters::DeployAgent { agent_version }
    | AnsibleParameters::UpdateAgent { agent_version } = &job.parameters
    {
        vars["lxcup_agent_version"] = serde_json::json!(agent_version);
        vars["lxcup_agent_binary_src"] = serde_json::json!(artifact(r, agent_version, dir).await?)
    };
    let vars_file = dir.join("vars.json");
    private(&vars_file, &vars.to_string()).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    job.transition_to(AnsibleJobStatus::Applying)
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    let playbook = playbook(job.operation, target.kind).ok_or(JobFailureCode::PlaybookFailed)?;
    let out = timeout(
        Duration::from_secs(900),
        Command::new("ansible-playbook")
            .current_dir("/opt/lxcup/ansible")
            .args([
                "-i",
                inventory
                    .to_str()
                    .ok_or(JobFailureCode::WorkerUnavailable)?,
                playbook,
                "--extra-vars",
                &format!("@{}", vars_file.display()),
            ])
            .output(),
    )
    .await
    .map_err(|_| JobFailureCode::Timeout)?
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    if !out.status.success() {
        return Err(JobFailureCode::PlaybookFailed);
    };
    Ok(!String::from_utf8_lossy(&out.stdout).contains("changed=0"))
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
