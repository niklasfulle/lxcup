use super::{
    AnsibleJob, AnsibleJobStatus, InvocationContext, JobEventKind, JobFailureCode, Repositories,
    apply_summary, check_mode_summary, event, finalize_reconciliation_source,
    finished_task_results, plan_summary, read_package_inventory, reconciliation_decision,
    reconciliation_summary, redact_output,
};
use std::{path::Path, process::Output};

pub(super) async fn mark_job_applying(
    repos: &Repositories,
    job: &mut AnsibleJob,
) -> Result<(), JobFailureCode> {
    job.transition_to(AnsibleJobStatus::Applying)
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
            status: AnsibleJobStatus::Applying,
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)
}

pub(super) async fn emit_worker_streams(
    repos: &Repositories,
    job_id: lxcup_core::AnsibleJobId,
    output: &Output,
    context: &InvocationContext,
) -> Result<(), JobFailureCode> {
    let redactions = [
        Some(context.credential.expose()),
        context.agent_token.as_ref().map(|value| value.expose()),
    ];
    for (source, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let message = redact_output(&String::from_utf8_lossy(bytes), &redactions);
        if message.trim().is_empty() {
            continue;
        }
        event(
            repos,
            job_id,
            JobEventKind::WorkerLog {
                source: source.to_owned(),
                message,
            },
        )
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    }
    Ok(())
}

pub(super) async fn emit_mode_summary(
    repos: &Repositories,
    job: &AnsibleJob,
    output: &Output,
) -> Result<(), JobFailureCode> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    match job.mode {
        lxcup_ansible::ExecutionMode::Apply => {
            emit_finished_tasks(repos, job.id, &stdout).await?;
            emit_log(
                repos,
                job.id,
                "apply",
                apply_summary(&stdout, output.status.success()),
            )
            .await?;
        }
        lxcup_ansible::ExecutionMode::Check => {
            emit_log(repos, job.id, "check", check_mode_summary(&stdout)).await?;
        }
        lxcup_ansible::ExecutionMode::Plan => {
            emit_log(repos, job.id, "plan", plan_summary(&stdout)).await?;
        }
        lxcup_ansible::ExecutionMode::Reconcile => {}
    }
    Ok(())
}

async fn emit_finished_tasks(
    repos: &Repositories,
    job_id: lxcup_core::AnsibleJobId,
    stdout: &str,
) -> Result<(), JobFailureCode> {
    for (task, changed) in finished_task_results(stdout) {
        event(repos, job_id, JobEventKind::TaskFinished { task, changed })
            .await
            .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    }
    Ok(())
}

async fn emit_log(
    repos: &Repositories,
    job_id: lxcup_core::AnsibleJobId,
    source: &str,
    message: String,
) -> Result<(), JobFailureCode> {
    event(
        repos,
        job_id,
        JobEventKind::WorkerLog {
            source: source.to_owned(),
            message,
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)
}

pub(super) async fn reconcile_job(
    repos: &Repositories,
    job_id: lxcup_core::AnsibleJobId,
    source: AnsibleJob,
    output: &Output,
) -> Result<(), JobFailureCode> {
    if !output.status.success() {
        return emit_log(
            repos,
            job_id,
            "reconcile",
            "Ist-Zustand konnte nicht zuverlässig ermittelt werden. Der Ursprungsjob bleibt abgleichspflichtig; dieser Reconcile-Lauf kann nach Behebung der Verbindungsursache erneut versucht werden.".to_owned(),
        )
        .await;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let decision = reconciliation_decision(&stdout);
    let message = reconciliation_summary(decision);
    emit_log(repos, job_id, "reconcile", message.clone()).await?;
    finalize_reconciliation_source(repos, source, decision, &message).await
}

pub(super) async fn persist_package_inventory(
    repos: &Repositories,
    job_id: lxcup_core::AnsibleJobId,
    dir: &Path,
    target_id: lxcup_core::TargetId,
) -> Result<(), JobFailureCode> {
    let snapshot = read_package_inventory(dir, target_id)?;
    repos
        .package_inventory
        .replace(&snapshot)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(
        repos,
        job_id,
        JobEventKind::TaskFinished {
            task: format!("package inventory: {} packages", snapshot.packages.len()),
            changed: false,
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)
}
