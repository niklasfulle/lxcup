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
    Ok(playbook_changed(&String::from_utf8_lossy(&out.stdout)))
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
mod tests {
    use super::{classify_playbook_failure, playbook_changed, *};
    use lxcup_ansible::{AnsibleJobRequest, ExecutionMode};
    use lxcup_core::{ActorRole, ResourceLifecycle, SecretId, SecretScope};
    use lxcup_secrets::CreateSecret;
    use std::time::Duration;

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
    fn package_inventory_normalization_preserves_versions_and_metadata() {
        let packages = normalize_package_inventory(serde_json::json!({
            "curl": [{"version": "8.5.0-2", "arch": "amd64", "source": "apt"}],
            "zlib1g": [{"version": "1:1.2.13", "architecture": "amd64"}]
        }))
        .unwrap();

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name.as_str(), "curl");
        assert_eq!(packages[0].architecture.as_deref(), Some("amd64"));
        assert_eq!(packages[1].version.as_str(), "1:1.2.13");
        assert!(normalize_package_inventory(serde_json::json!([])).is_err());
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
            r#"{"packages":{"curl":[{"version":"8.5.0","arch":"amd64"}]}}"#,
        )
        .unwrap();
        let snapshot = read_package_inventory(&root, target_id).unwrap();
        assert_eq!(snapshot.target_id, target_id);
        assert_eq!(snapshot.packages.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn artifact_download_checks_version_platform_and_sha256() {
        let root = std::env::temp_dir().join(format!("lxcup-worker-artifact-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let binary = b"verified test agent".to_vec();
        let digest = format!("{:x}", Sha256::digest(&binary));
        let base_manifest = serde_json::json!({
            "version": "0.2.0",
            "artifacts": [{"platform": "linux-amd64", "file": "agent", "sha256": digest}]
        });

        let (base, server) = artifact_server(base_manifest.clone(), binary.clone(), 2).await;
        let mut runtime = runtime_with_secrets(&root).0;
        runtime.artifacts = base;
        let path = artifact(&runtime, "0.2.0", &root).await.unwrap();
        assert_eq!(fs::read(path).unwrap(), binary);
        server.await.unwrap();

        let mut wrong_version = base_manifest.clone();
        wrong_version["version"] = serde_json::json!("0.1.0");
        let (base, server) = artifact_server(wrong_version, Vec::new(), 1).await;
        runtime.artifacts = base;
        assert_eq!(
            artifact(&runtime, "0.2.0", &root).await,
            Err(JobFailureCode::PlaybookFailed)
        );
        server.await.unwrap();

        let mut wrong_hash = base_manifest;
        wrong_hash["artifacts"][0]["sha256"] = serde_json::json!("00");
        let (base, server) = artifact_server(wrong_hash, binary, 2).await;
        runtime.artifacts = base;
        assert_eq!(
            artifact(&runtime, "0.2.0", &root).await,
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
}
