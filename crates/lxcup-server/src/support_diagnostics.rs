use super::{ApiEnvelope, ApiError, ApiState, envelope};
use axum::{
    Extension, Json,
    extract::{Query, State},
};
use chrono::{DateTime, Duration, Utc};
use lxcup_ansible::{AnsibleJob, JobEvent, JobEventKind};
use lxcup_core::{ActorRole, ResourceTarget, Target, TargetId};
use serde::Serialize;

const MAX_TARGETS: usize = 250;
const MAX_JOBS: usize = 50;
const MAX_EVENTS_PER_JOB: usize = 100;
const MAX_LOG_SUMMARIES_PER_JOB: usize = 10;
const MAX_SUMMARY_CHARS: usize = 500;
const STALE_AFTER: Duration = Duration::minutes(2);
const DEFAULT_PERIOD_DAYS: u8 = 7;
const MAX_PERIOD_DAYS: u8 = 30;

#[derive(serde::Deserialize)]
pub(crate) struct DiagnosticsQuery {
    #[serde(default = "default_period_days")]
    days: u8,
}

#[derive(Serialize)]
pub(crate) struct SupportDiagnostics {
    schema_version: u8,
    generated_at: DateTime<Utc>,
    period_start: DateTime<Utc>,
    versions: VersionInfo,
    worker: WorkerInfo,
    resources: Vec<ResourceInfo>,
    workflows: Vec<WorkflowInfo>,
    privacy: PrivacyInfo,
}

#[derive(Serialize)]
struct VersionInfo {
    controller: &'static str,
    agent_artifact: &'static str,
    artifact_store: Option<&'static str>,
    worker: Option<String>,
}

#[derive(Serialize)]
struct WorkerInfo {
    available: bool,
    worker_version: Option<String>,
    last_seen_at: Option<DateTime<Utc>>,
    artifact_store_available: Option<bool>,
    artifact_store_checked_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct ResourceInfo {
    id: TargetId,
    name: String,
    kind: String,
    address: String,
    state: String,
    agent_platform: Option<String>,
    agent_hostname: Option<String>,
    agent_version: Option<String>,
    heartbeat_at: Option<DateTime<Utc>>,
    heartbeat_stale: bool,
}

#[derive(Serialize)]
struct WorkflowInfo {
    id: lxcup_core::AnsibleJobId,
    operation: String,
    target: String,
    mode: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    failure_codes: Vec<String>,
    log_summaries: Vec<LogSummary>,
}

#[derive(Serialize)]
struct LogSummary {
    at: DateTime<Utc>,
    source: String,
    message: String,
}

#[derive(Serialize)]
struct PrivacyInfo {
    secrets_included: bool,
    raw_worker_logs_included: bool,
    external_upload: bool,
    note: &'static str,
}

pub(crate) async fn get_support_diagnostics(
    State(state): State<ApiState>,
    Extension(role): Extension<ActorRole>,
    Query(query): Query<DiagnosticsQuery>,
) -> Result<Json<ApiEnvelope<SupportDiagnostics>>, ApiError> {
    if role != ActorRole::Admin {
        return Err(ApiError::forbidden(
            "admin_required",
            "administrator access is required for support diagnostics",
        ));
    }
    if !(1..=MAX_PERIOD_DAYS).contains(&query.days) {
        return Err(ApiError::bad_request(
            "invalid_diagnostics_period",
            "diagnostic period must be between 1 and 30 days",
        ));
    }

    let now = Utc::now();
    let period_start = now - Duration::days(i64::from(query.days));
    let (targets, reports) = {
        let store = state.store.read().await;
        (store.targets.clone(), store.agent_reports.clone())
    };
    let mut resources = targets
        .iter()
        .take(MAX_TARGETS)
        .map(|target| resource_info(target, reports.get(&target.id), now))
        .collect::<Vec<_>>();
    resources.sort_by(|left, right| left.name.cmp(&right.name));

    let (worker, jobs) =
        tokio::try_join!(worker_info(&state, now), latest_jobs(&state, period_start))?;
    let mut workflows = Vec::with_capacity(jobs.len());
    for job in jobs {
        let events = job_events(&state, job.id, period_start).await?;
        workflows.push(workflow_info(&job, &events, period_start));
    }

    Ok(Json(envelope(SupportDiagnostics {
        schema_version: 1,
        generated_at: now,
        period_start,
        versions: VersionInfo {
            controller: env!("CARGO_PKG_VERSION"),
            agent_artifact: env!("CARGO_PKG_VERSION"),
            artifact_store: worker
                .artifact_store_available
                .is_some_and(|available| available)
                .then_some(env!("CARGO_PKG_VERSION")),
            worker: worker.worker_version.clone(),
        },
        worker,
        resources,
        workflows,
        privacy: PrivacyInfo {
            secrets_included: false,
            raw_worker_logs_included: false,
            external_upload: false,
            note: "Only allowlisted workflow summaries are included. This file is generated locally and is not uploaded.",
        },
    })))
}

fn resource_info(
    target: &Target,
    report: Option<&lxcup_agent::AgentHeartbeat>,
    now: DateTime<Utc>,
) -> ResourceInfo {
    let heartbeat_at = report.map(|heartbeat| heartbeat.sent_at);
    ResourceInfo {
        id: target.id,
        name: diagnostic_text(&target.name),
        kind: format!("{:?}", target.kind).to_ascii_lowercase(),
        address: diagnostic_text(&target.address),
        state: format!("{:?}", target.state).to_ascii_lowercase(),
        agent_platform: report
            .map(|heartbeat| format!("{:?}", heartbeat.info.platform).to_ascii_lowercase()),
        agent_hostname: report.map(|heartbeat| diagnostic_text(&heartbeat.info.hostname)),
        agent_version: report.map(|heartbeat| diagnostic_text(&heartbeat.info.version)),
        heartbeat_at,
        heartbeat_stale: heartbeat_at.is_none_or(|sent_at| sent_at < now - STALE_AFTER),
    }
}

async fn worker_info(state: &ApiState, now: DateTime<Utc>) -> Result<WorkerInfo, ApiError> {
    let Some(repositories) = state.repositories.as_ref() else {
        return Ok(WorkerInfo {
            available: false,
            worker_version: None,
            last_seen_at: None,
            artifact_store_available: None,
            artifact_store_checked_at: None,
        });
    };
    let latest = repositories
        .worker_heartbeats
        .latest_status()
        .await
        .map_err(|_| ApiError::storage())?;
    Ok(WorkerInfo {
        available: latest
            .as_ref()
            .is_some_and(|status| status.last_seen_at >= now - Duration::seconds(10)),
        worker_version: latest
            .as_ref()
            .and_then(|status| status.worker_version.as_deref())
            .map(diagnostic_text),
        last_seen_at: latest.as_ref().map(|status| status.last_seen_at),
        artifact_store_available: latest
            .as_ref()
            .and_then(|status| status.artifact_store_available),
        artifact_store_checked_at: latest.and_then(|status| status.artifact_store_checked_at),
    })
}

async fn latest_jobs(
    state: &ApiState,
    period_start: DateTime<Utc>,
) -> Result<Vec<AnsibleJob>, ApiError> {
    let mut jobs = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .ansible_jobs
            .list_since(period_start, MAX_JOBS as u32)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    jobs.retain(|job| job.created_at >= period_start);
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
    jobs.truncate(MAX_JOBS);
    Ok(jobs)
}

async fn job_events(
    state: &ApiState,
    id: lxcup_core::AnsibleJobId,
    since: DateTime<Utc>,
) -> Result<Vec<JobEvent>, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .ansible_jobs
            .events_since(id, since, MAX_EVENTS_PER_JOB as u32)
            .await
            .map_err(|_| ApiError::storage())
    } else {
        let mut events = state
            .ansible
            .read()
            .await
            .events(id)
            .map_err(|_| ApiError::storage())?
            .into_iter()
            .filter(|event| event.created_at >= since)
            .collect::<Vec<_>>();
        if events.len() > MAX_EVENTS_PER_JOB {
            events.drain(..events.len() - MAX_EVENTS_PER_JOB);
        }
        Ok(events)
    }
}

fn workflow_info(
    job: &AnsibleJob,
    events: &[JobEvent],
    period_start: DateTime<Utc>,
) -> WorkflowInfo {
    let mut failure_codes = Vec::new();
    let mut log_summaries = Vec::new();
    for event in events
        .iter()
        .filter(|event| event.created_at >= period_start)
        .rev()
    {
        match &event.event {
            JobEventKind::Failed { code } => {
                failure_codes.push(format!("{code:?}").to_ascii_lowercase())
            }
            JobEventKind::WorkerLog { source, message }
                if matches!(source.as_str(), "apply" | "check" | "plan" | "reconcile") =>
            {
                let message = sanitize_summary(message);
                if !message.is_empty() && log_summaries.len() < MAX_LOG_SUMMARIES_PER_JOB {
                    log_summaries.push(LogSummary {
                        at: event.created_at,
                        source: source.clone(),
                        message,
                    });
                }
            }
            _ => {}
        }
    }
    failure_codes.dedup();
    log_summaries.reverse();
    WorkflowInfo {
        id: job.id,
        operation: format!("{:?}", job.operation).to_ascii_lowercase(),
        target: target_label(job.target),
        mode: format!("{:?}", job.mode).to_ascii_lowercase(),
        status: format!("{:?}", job.status).to_ascii_lowercase(),
        created_at: job.created_at,
        updated_at: job.updated_at,
        failure_codes,
        log_summaries,
    }
}

fn target_label(target: ResourceTarget) -> String {
    match target {
        ResourceTarget::Target(id) => format!("resource:{}", id.as_uuid()),
        ResourceTarget::Node(id) => format!("node:{}", id.as_uuid()),
        ResourceTarget::Container(id) => format!("container:{}", id.value()),
    }
}

fn sanitize_summary(message: &str) -> String {
    let mut inside_pem_block = false;
    message
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            if inside_pem_block {
                if lower.contains("-----end") {
                    inside_pem_block = false;
                }
                return false;
            }
            if lower.contains("-----begin") {
                inside_pem_block = true;
                return false;
            }
            !contains_sensitive_material(line)
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect()
}

fn diagnostic_text(value: &str) -> String {
    let sanitized = sanitize_summary(value);
    if sanitized.is_empty() && !value.is_empty() {
        "[REDACTED]".to_owned()
    } else {
        sanitized
    }
}

fn contains_sensitive_material(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let markers = [
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "api-key",
        "api key",
        "access_key",
        "access-key",
        "access key",
        "private key",
        "private-key",
        "authorization",
        "bearer ",
        "-----begin",
    ];
    markers.iter().any(|marker| lower.contains(marker))
        || has_url_userinfo(line)
        || line
            .split(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
            })
            .any(is_credential_token)
}

fn has_url_userinfo(line: &str) -> bool {
    let rest = line.split_once("://").map_or(line, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#', ' ']).next().unwrap_or_default();
    authority
        .split_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'))
}

fn is_credential_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let github_prefixes = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    if github_prefixes.iter().any(|prefix| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() >= 10)
    }) {
        return true;
    }
    if token.starts_with("AKIA")
        && token.as_bytes().get(4..20).is_some_and(|key| {
            key.iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
    {
        return true;
    }
    if lower.starts_with("xox") || lower.starts_with("ya29.") {
        return true;
    }
    let mut jwt_parts = token.split('.');
    matches!(
        (
            jwt_parts.next(),
            jwt_parts.next(),
            jwt_parts.next(),
            jwt_parts.next()
        ),
        (Some(header), Some(payload), Some(signature), None)
            if header.starts_with("eyJ")
                && payload.len() >= 8
                && signature.len() >= 8
                && token.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                })
    )
}

const fn default_period_days() -> u8 {
    DEFAULT_PERIOD_DAYS
}

#[cfg(test)]
#[path = "support_diagnostics_tests.rs"]
mod tests;
