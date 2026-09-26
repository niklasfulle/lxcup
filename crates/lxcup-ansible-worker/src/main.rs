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

mod execution_support;

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
                            task: if job.mode == lxcup_ansible::ExecutionMode::Apply {
                                "Apply-Ausführung".to_owned()
                            } else {
                                job.playbook.clone()
                            },
                            changed,
                        },
                    )
                }
                Err(code) => {
                    tracing::error!(failure_code = ?code, "workflow job execution failed");
                    let status = if job.mode == lxcup_ansible::ExecutionMode::Apply
                        && job.status == AnsibleJobStatus::Applying
                    {
                        AnsibleJobStatus::ReconcileRequired
                    } else {
                        AnsibleJobStatus::Failed
                    };
                    (status, JobEventKind::Failed { code })
                }
            };
            job.transition_to(status).expect("claimed status");
            repos.ansible_jobs.update(&job).await?;
            event(repos, job.id, job_event).await?;
            if status == AnsibleJobStatus::ReconcileRequired {
                event(repos, job.id, JobEventKind::ReconcileRequired).await?;
            }
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
    let reconciliation_source = if job.mode == lxcup_ansible::ExecutionMode::Reconcile {
        Some(load_reconciliation_source(repos, job).await?)
    } else {
        None
    };
    let context = prepare_invocation(r, job, target, dir).await?;
    if job.mode == lxcup_ansible::ExecutionMode::Apply {
        execution_support::mark_job_applying(repos, job).await?;
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
    execution_support::emit_worker_streams(repos, job.id, &out, &context).await?;
    execution_support::emit_mode_summary(repos, job, &out).await?;
    if let Some(source) = reconciliation_source {
        execution_support::reconcile_job(repos, job.id, source, &out).await?;
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
        execution_support::persist_package_inventory(repos, job.id, dir, target.id).await?;
    }
    Ok(job.mode == lxcup_ansible::ExecutionMode::Apply
        && playbook_changed(&String::from_utf8_lossy(&out.stdout)))
}

fn ansible_mode_args(
    operation: AnsibleOperation,
    mode: lxcup_ansible::ExecutionMode,
) -> Result<&'static [&'static str], JobFailureCode> {
    let dedicated_reconciliation =
        mode == lxcup_ansible::ExecutionMode::Reconcile && operation.can_reconcile_apply();
    if !operation.supports_mode(mode) && !dedicated_reconciliation {
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
        lxcup_ansible::ExecutionMode::Plan => match operation {
            AnsibleOperation::DeployAgent
            | AnsibleOperation::UpdateAgent
            | AnsibleOperation::RepairAgent => Ok(["--check", "--diff"].as_slice()),
            AnsibleOperation::UpdatePackages => Ok(["--diff"].as_slice()),
            _ => Err(JobFailureCode::PlaybookFailed),
        },
        lxcup_ansible::ExecutionMode::Apply => Ok(&[]),
        lxcup_ansible::ExecutionMode::Reconcile => match operation {
            AnsibleOperation::DeployAgent
            | AnsibleOperation::UpdateAgent
            | AnsibleOperation::RepairAgent => Ok(["--check", "--diff"].as_slice()),
            AnsibleOperation::UpdatePackages => Ok(["--diff"].as_slice()),
            _ => Err(JobFailureCode::PlaybookFailed),
        },
    }
}

fn check_mode_summary(stdout: &str) -> String {
    if !stdout.contains("PLAY RECAP") {
        "Prüfung lieferte keine vollständige Ansible-Zusammenfassung; Ergebnis bitte manuell prüfen.".to_owned()
    } else if recap_has_failures(stdout) {
        "Prüfung fehlgeschlagen: Ansible meldet mindestens einen fehlgeschlagenen oder nicht erreichbaren Host. Details stehen in der Worker-Ausgabe.".to_owned()
    } else if stdout
        .lines()
        .any(|line| line.trim_start().starts_with("skipping:"))
    {
        "Prüfung abgeschlossen, aber mindestens ein Ansible-Schritt wurde übersprungen und ist nicht prüfbar. Details stehen in der Worker-Ausgabe.".to_owned()
    } else if playbook_changed(stdout) {
        "Prüfung erfolgreich: Ansible hat mögliche Änderungen erkannt, aber im Check-Modus nichts angewendet.".to_owned()
    } else {
        "Prüfung erfolgreich: Ansible hat keine Änderungen erkannt und nichts angewendet."
            .to_owned()
    }
}

fn plan_summary(stdout: &str) -> String {
    if !stdout.contains("PLAY RECAP") {
        return "Vorschau unvollständig: Ansible hat keine vollständige Zusammenfassung geliefert. Details stehen in der technischen Ausgabe.".to_owned();
    }
    if recap_has_failures(stdout) {
        return "Vorschau fehlgeschlagen: Ansible meldet mindestens einen fehlgeschlagenen oder nicht erreichbaren Host. Es wurde nichts angewendet; Details stehen in der technischen Ausgabe.".to_owned();
    }
    if stdout
        .lines()
        .any(|line| line.trim_start().starts_with("skipping:"))
    {
        return "Vorschau teilweise nicht prüfbar: Mindestens ein Ansible-Schritt wurde übersprungen. Es wurden keine Änderungen angewendet; Details stehen in der Diff-Ausgabe.".to_owned();
    }
    let changes = stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("changed=")?.parse::<u32>().ok())
        })
        .sum::<u32>();
    if changes == 0 {
        "Dry-Run erfolgreich: Es werden keine Änderungen erwartet. Es wurde nichts angewendet."
            .to_owned()
    } else {
        format!(
            "Dry-Run erfolgreich: Ansible erwartet {changes} Änderung(en). Es wurde nichts angewendet; die sichere Diff-Vorschau steht unten."
        )
    }
}

fn recap_has_failures(stdout: &str) -> bool {
    stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .any(|line| {
            line.split_whitespace().any(|field| {
                ["failed=", "unreachable="]
                    .iter()
                    .any(|prefix| field.strip_prefix(prefix).is_some_and(|value| value != "0"))
            })
        })
}

fn recap_has_skipped_tasks(stdout: &str) -> bool {
    stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .any(|line| {
            line.split_whitespace().any(|field| {
                field
                    .strip_prefix("skipped=")
                    .and_then(|value| value.parse::<u32>().ok())
                    .is_some_and(|count| count > 0)
            })
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconciliationDecision {
    Completed,
    ChangesRemain,
    ManualReview,
}

fn reconciliation_decision(stdout: &str) -> ReconciliationDecision {
    if !stdout.contains("PLAY RECAP")
        || recap_has_failures(stdout)
        || recap_has_skipped_tasks(stdout)
        || stdout
            .lines()
            .any(|line| line.trim_start().starts_with("skipping:"))
    {
        return ReconciliationDecision::ManualReview;
    }
    match recap_change_count(stdout) {
        Some(0) => ReconciliationDecision::Completed,
        Some(_) => ReconciliationDecision::ChangesRemain,
        None => ReconciliationDecision::ManualReview,
    }
}

fn reconciliation_summary(decision: ReconciliationDecision) -> String {
    match decision {
        ReconciliationDecision::Completed => "Ist-Zustand geprüft: Es sind keine Änderungen mehr offen. Der ursprüngliche Apply-Lauf wird als abgeschlossen markiert; Reconcile hat selbst nichts geändert.".to_owned(),
        ReconciliationDecision::ChangesRemain => "Ist-Zustand geprüft: Es bleiben Änderungen offen. Der ursprüngliche Lauf wird als fehlgeschlagen markiert. Erstelle einen neuen Plan und bestätige einen neuen Apply ausdrücklich; Reconcile hat selbst nichts geändert.".to_owned(),
        ReconciliationDecision::ManualReview => "Ist-Zustand konnte nicht vollständig bewertet werden. Der ursprüngliche Lauf wird zur manuellen Prüfung als fehlgeschlagen markiert; Reconcile hat selbst nichts geändert.".to_owned(),
    }
}

async fn load_reconciliation_source(
    repos: &Repositories,
    job: &AnsibleJob,
) -> Result<AnsibleJob, JobFailureCode> {
    let source_id = lxcup_ansible::reconciliation_source_job_id(&job.idempotency_key)
        .ok_or(JobFailureCode::PlaybookFailed)?;
    let source = repos
        .ansible_jobs
        .find_by_id(source_id)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .ok_or(JobFailureCode::PlaybookFailed)?;
    if source.status != AnsibleJobStatus::ReconcileRequired
        || source.mode != lxcup_ansible::ExecutionMode::Apply
        || !source.operation.can_reconcile_apply()
        || source.target != job.target
        || source.operation != job.operation
        || source.parameters != job.parameters
        || source.secret_refs != job.secret_refs
    {
        return Err(JobFailureCode::PlaybookFailed);
    }
    Ok(source)
}

async fn finalize_reconciliation_source(
    repos: &Repositories,
    mut source: AnsibleJob,
    decision: ReconciliationDecision,
    message: &str,
) -> Result<(), JobFailureCode> {
    let status = if decision == ReconciliationDecision::Completed {
        AnsibleJobStatus::Succeeded
    } else {
        AnsibleJobStatus::Failed
    };
    source
        .transition_to(status)
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    repos
        .ansible_jobs
        .update(&source)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(
        repos,
        source.id,
        JobEventKind::WorkerLog {
            source: "reconcile".to_owned(),
            message: message.to_owned(),
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(repos, source.id, JobEventKind::StatusChanged { status })
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(())
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

fn recap_change_count(stdout: &str) -> Option<u32> {
    let mut found = false;
    let changes = stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .flat_map(str::split_whitespace)
        .filter_map(|field| {
            let value = field.strip_prefix("changed=")?.parse::<u32>().ok()?;
            found = true;
            Some(value)
        })
        .sum();
    found.then_some(changes)
}

fn playbook_changed(stdout: &str) -> bool {
    recap_change_count(stdout)
        .map(|changes| changes > 0)
        .unwrap_or_else(|| {
            stdout
                .lines()
                .any(|line| line.trim_start().starts_with("changed: ["))
        })
}

fn finished_task_results(stdout: &str) -> Vec<(String, bool)> {
    let mut results = Vec::new();
    let mut current: Option<(String, bool, bool)> = None;
    for line in stdout.lines() {
        if let Some(task) = task_header_name(line) {
            push_finished_task(&mut current, &mut results);
            current = Some((task.to_owned(), false, false));
            continue;
        }
        update_task_result(&mut current, line);
    }
    push_finished_task(&mut current, &mut results);
    results
}

fn push_finished_task(
    current: &mut Option<(String, bool, bool)>,
    results: &mut Vec<(String, bool)>,
) {
    if let Some((name, changed, finished)) = current {
        if *finished {
            results.push((std::mem::take(name), *changed));
        }
    }
    *current = None;
}

fn update_task_result(current: &mut Option<(String, bool, bool)>, line: &str) {
    let Some((_, changed, finished)) = current.as_mut() else {
        return;
    };
    let outcome = line.trim_start();
    if outcome.starts_with("changed: [") {
        *changed = true;
        *finished = true;
    } else if is_finished_outcome(outcome) {
        *finished = true;
    }
}

fn is_finished_outcome(outcome: &str) -> bool {
    ["ok: [", "fatal: [", "unreachable: ["]
        .iter()
        .any(|prefix| outcome.starts_with(prefix))
}

fn task_header_name(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let header = line
        .strip_prefix("TASK [")
        .or_else(|| line.strip_prefix("RUNNING HANDLER ["))?;
    header.split_once(']').map(|(name, _)| name)
}

fn apply_summary(stdout: &str, process_succeeded: bool) -> String {
    let changed_tasks = finished_task_results(stdout)
        .iter()
        .filter(|(_, changed)| *changed)
        .count();
    if !process_succeeded {
        format!(
            "Apply fehlgeschlagen. Bis zum Fehler haben {changed_tasks} Task(s) Änderungen gemeldet; Details und Status je Task stehen im Protokoll."
        )
    } else if changed_tasks > 0 {
        format!(
            "Apply erfolgreich ausgeführt: {changed_tasks} Task(s) haben Änderungen vorgenommen. Die tatsächlichen Schritte stehen im Protokoll."
        )
    } else if playbook_changed(stdout) {
        format!(
            "Apply erfolgreich ausgeführt: Ansible meldet {} Änderung(en); einzelne Task-Ausgaben stehen im technischen Protokoll.",
            recap_change_count(stdout).unwrap_or_default()
        )
    } else {
        "Apply erfolgreich ausgeführt: Ansible meldet keine Änderungen am Zielsystem.".to_owned()
    }
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
    let execution_mode = if job.mode == lxcup_ansible::ExecutionMode::Reconcile {
        "plan"
    } else {
        match job.mode {
            lxcup_ansible::ExecutionMode::Check => "check",
            lxcup_ansible::ExecutionMode::Plan => "plan",
            lxcup_ansible::ExecutionMode::Apply => "apply",
            lxcup_ansible::ExecutionMode::Reconcile => unreachable!(),
        }
    };
    let mut vars = serde_json::json!({"lxcup_execution_mode": execution_mode});
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
    #[serde(default)]
    upgradable: Vec<String>,
}

fn read_package_inventory(
    dir: &Path,
    target_id: lxcup_core::TargetId,
) -> Result<PackageInventorySnapshot, JobFailureCode> {
    let contents = fs::read_to_string(dir.join("package-inventory.json"))
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    let collected: CollectedPackageInventory =
        serde_json::from_str(&contents).map_err(|_| JobFailureCode::PlaybookFailed)?;
    let packages = normalize_package_inventory(collected.packages, &collected.upgradable)?;
    Ok(PackageInventorySnapshot {
        target_id,
        collected_at: Utc::now(),
        packages,
    })
}

fn normalize_package_inventory(
    value: serde_json::Value,
    upgradable: &[String],
) -> Result<Vec<InstalledPackage>, JobFailureCode> {
    let candidate_versions = upgradable
        .iter()
        .filter_map(|line| parse_apt_upgrade(line))
        .collect::<std::collections::HashMap<_, _>>();
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
            let installed_version = PackageVersion::new(version.to_owned())
                .map_err(|_| JobFailureCode::PlaybookFailed)?;
            let candidate_version = candidate_versions
                .get(name)
                .or_else(|| {
                    name.split(':')
                        .next()
                        .and_then(|name| candidate_versions.get(name))
                })
                .map(|value| PackageVersion::new(value.clone()))
                .transpose()
                .map_err(|_| JobFailureCode::PlaybookFailed)?
                .unwrap_or_else(|| installed_version.clone());
            packages.push(InstalledPackage {
                name: PackageName::new(name.clone()).map_err(|_| JobFailureCode::PlaybookFailed)?,
                version: installed_version,
                candidate_version: Some(candidate_version),
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

fn parse_apt_upgrade(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("Listing") || line.starts_with("WARNING") {
        return None;
    }
    let mut fields = line.split_whitespace();
    let package = fields.next()?.split('/').next()?.split(':').next()?;
    let candidate = fields.next()?;
    if package.is_empty() || candidate.is_empty() || !line.contains("[upgradable from:") {
        return None;
    }
    Some((package.to_owned(), candidate.to_owned()))
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
