use super::{
    AgentWorkflowClaimRequest, AgentWorkflowCommand, AgentWorkflowResult, ApiEnvelope, ApiError,
    ApiEvent, ApiState, Json, JsonBody, Path, State, TargetId, TargetKind, TargetTransport,
    envelope, map_secret_error, parse_uuid,
};
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use lxcup_agent::{AgentAction, parse_windows_packages, safe_winget_id};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, ExecutionMode, JobEvent,
    JobEventKind,
};
use lxcup_core::ResourceTarget;

pub(crate) async fn claim_windows_agent_workflow(
    State(state): State<ApiState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<AgentWorkflowClaimRequest>,
) -> Result<Json<ApiEnvelope<Option<AgentWorkflowCommand>>>, ApiError> {
    let target_id = TargetId::from_uuid(request.target_id);
    authorize_windows_agent(&state, &headers, target_id).await?;
    let job = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .claim_next_windows_agent_job(target_id)
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        let mut coordinator = state.ansible.write().await;
        let candidate = coordinator.jobs().into_iter().find(|job| {
            job.target == ResourceTarget::Target(target_id)
                && job.status == AnsibleJobStatus::Queued
                && matches!(
                    job.operation,
                    AnsibleOperation::HealthCheck
                        | AnsibleOperation::CollectPackageInventory
                        | AnsibleOperation::UpdatePackages
                )
        });
        if let Some(candidate) = candidate {
            let mut claimed = coordinator
                .transition(candidate.id, AnsibleJobStatus::Checking)
                .map_err(|_| {
                    ApiError::conflict("ansible_target_busy", "target has an active workflow")
                })?;
            if candidate.mode == ExecutionMode::Apply {
                claimed = coordinator
                    .transition(candidate.id, AnsibleJobStatus::Planned)
                    .and_then(|_| coordinator.transition(candidate.id, AnsibleJobStatus::Applying))
                    .map_err(|_| {
                        ApiError::conflict("ansible_target_busy", "target has an active workflow")
                    })?;
            }
            Some(claimed)
        } else {
            None
        }
    };
    let command = if let Some(job) = job {
        match workflow_command(job.clone()) {
            Ok(command) => Some(command),
            Err(error) => {
                persist_result(
                    &state,
                    job.clone(),
                    AnsibleJobStatus::Failed,
                    error.message.to_string(),
                )
                .await?;
                state.publish(ApiEvent::status(
                    "ansible_job",
                    job.id.as_uuid().to_string(),
                    "failed",
                ));
                None
            }
        }
    } else {
        None
    };
    if let Some(command) = &command {
        state.publish(ApiEvent::status(
            "ansible_job",
            command.job_id.to_string(),
            if command.action == AgentAction::Apply {
                "applying"
            } else {
                "checking"
            },
        ));
    }
    Ok(Json(envelope(command)))
}

pub(crate) async fn report_windows_agent_workflow(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    JsonBody(result): JsonBody<AgentWorkflowResult>,
) -> Result<StatusCode, ApiError> {
    let job_id = lxcup_core::AnsibleJobId::from_uuid(parse_uuid(&job_id, "workflow id")?);
    let job = load_job(&state, job_id).await?;
    let ResourceTarget::Target(target_id) = job.target else {
        return Err(ApiError::not_found("workflow not found"));
    };
    authorize_windows_agent(&state, &headers, target_id).await?;
    if matches!(
        job.status,
        AnsibleJobStatus::Succeeded
            | AnsibleJobStatus::Failed
            | AnsibleJobStatus::Aborted
            | AnsibleJobStatus::ReconcileRequired
    ) {
        return Ok(StatusCode::NO_CONTENT);
    }
    if result.response.stdout.len() > 128 * 1024 || result.response.stderr.len() > 16 * 1024 {
        return Err(ApiError::bad_request(
            "agent_result_too_large",
            "workflow result exceeds the allowed size",
        ));
    }
    let (log_message, usable_result) = result_summary(&job, &result);
    let succeeded = result.response.success && usable_result;
    let final_status = match (job.mode, succeeded) {
        (ExecutionMode::Apply, true) => AnsibleJobStatus::Succeeded,
        (ExecutionMode::Apply, false) => AnsibleJobStatus::ReconcileRequired,
        (ExecutionMode::Plan, true) if job.operation == AnsibleOperation::UpdatePackages => {
            AnsibleJobStatus::Succeeded
        }
        (ExecutionMode::Plan, false) => AnsibleJobStatus::Failed,
        (_, true) => AnsibleJobStatus::Succeeded,
        (_, false) => AnsibleJobStatus::Failed,
    };
    persist_result(&state, job, final_status, log_message).await?;
    state.publish(ApiEvent::status(
        "ansible_job",
        job_id.as_uuid().to_string(),
        status_name(final_status),
    ));
    Ok(StatusCode::NO_CONTENT)
}

fn workflow_command(job: AnsibleJob) -> Result<AgentWorkflowCommand, ApiError> {
    let action = match (job.operation, job.mode) {
        (AnsibleOperation::HealthCheck, _) => AgentAction::Health,
        (AnsibleOperation::CollectPackageInventory, _) => AgentAction::Scan,
        (AnsibleOperation::UpdatePackages, ExecutionMode::Apply) => AgentAction::Apply,
        (AnsibleOperation::UpdatePackages, ExecutionMode::Plan | ExecutionMode::Reconcile) => {
            AgentAction::Scan
        }
        _ => {
            return Err(ApiError::bad_request(
                "windows_workflow_unsupported",
                "this operation is not supported by the local Windows agent",
            ));
        }
    };
    let packages = match job.parameters {
        AnsibleParameters::UpdatePackages { packages } => {
            if packages.is_empty() || packages.iter().any(|package| !safe_winget_id(package)) {
                return Err(ApiError::bad_request(
                    "invalid_winget_package_id",
                    "Windows updates require exact winget package IDs",
                ));
            }
            packages
        }
        _ => Vec::new(),
    };
    Ok(AgentWorkflowCommand {
        job_id: job.id.as_uuid(),
        action,
        packages,
    })
}

async fn authorize_windows_agent(
    state: &ApiState,
    headers: &HeaderMap,
    target_id: TargetId,
) -> Result<(), ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let store = state.store.read().await;
    let target = store
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    if target.kind != TargetKind::WindowsServer || target.transport != TargetTransport::Agent {
        return Err(ApiError::bad_request(
            "windows_agent_required",
            "workflow must be claimed by a locally installed Windows agent",
        ));
    }
    let expected = state
        .secrets
        .read(target.agent_secret_ref)
        .map_err(map_secret_error)?;
    if expected.expose() != token {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}

async fn load_job(state: &ApiState, id: lxcup_core::AnsibleJobId) -> Result<AnsibleJob, ApiError> {
    if let Some(repositories) = state.repositories.clone() {
        return repositories
            .ansible_jobs
            .find_by_id(id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("workflow not found"));
    }
    state
        .ansible
        .read()
        .await
        .job(id)
        .map_err(super::workflows::map_ansible_error)
}

fn result_summary(job: &AnsibleJob, result: &AgentWorkflowResult) -> (String, bool) {
    if !result.response.success {
        return (
            format!(
                "Windows agent reported a local command failure (exit code {}).",
                result.response.exit_code
            ),
            false,
        );
    }
    if job.operation == AnsibleOperation::UpdatePackages && job.mode == ExecutionMode::Plan {
        let Ok(packages) = parse_windows_packages(&result.response.stdout) else {
            return (
                "winget update scan returned an invalid or unsupported listing.".to_owned(),
                false,
            );
        };
        let AnsibleParameters::UpdatePackages {
            packages: requested,
        } = &job.parameters
        else {
            return ("package update parameters were invalid.".to_owned(), false);
        };
        let lines = packages
            .iter()
            .filter(|package| {
                requested
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&package.name))
            })
            .filter_map(|package| {
                package.candidate_version.as_ref().map(|candidate| {
                    format!(
                        "{}: {} -> {}",
                        package.name, package.installed_version, candidate
                    )
                })
            })
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return (
                "winget found no available updates for the selected package IDs.".to_owned(),
                false,
            );
        }
        return (format!("winget update plan:\n{}", lines.join("\n")), true);
    }
    let action = match job.operation {
        AnsibleOperation::HealthCheck => "Windows agent health check completed successfully.",
        AnsibleOperation::CollectPackageInventory => {
            "Windows software inventory collected locally with winget."
        }
        AnsibleOperation::UpdatePackages if job.mode == ExecutionMode::Apply => {
            "Selected updates were installed locally by winget."
        }
        _ => "Windows agent workflow completed successfully.",
    };
    (action.to_owned(), true)
}

async fn persist_result(
    state: &ApiState,
    mut job: AnsibleJob,
    status: AnsibleJobStatus,
    message: String,
) -> Result<(), ApiError> {
    let mut statuses = Vec::new();
    if status == AnsibleJobStatus::Succeeded && job.mode != ExecutionMode::Apply {
        job.transition_to(AnsibleJobStatus::Planned).map_err(|_| {
            ApiError::conflict(
                "invalid_workflow_state",
                "workflow is not awaiting an agent result",
            )
        })?;
        statuses.push(AnsibleJobStatus::Planned);
        job.transition_to(AnsibleJobStatus::Succeeded)
            .map_err(|_| {
                ApiError::conflict("invalid_workflow_state", "workflow cannot be completed")
            })?;
        statuses.push(AnsibleJobStatus::Succeeded);
    } else {
        job.transition_to(status).map_err(|_| {
            ApiError::conflict(
                "invalid_workflow_state",
                "workflow is not awaiting an agent result",
            )
        })?;
        statuses.push(status);
    }
    if let Some(repositories) = state.repositories.clone() {
        let previous = repositories
            .ansible_jobs
            .events(job.id)
            .await
            .map_err(|_| ApiError::storage())?;
        repositories
            .ansible_jobs
            .update(&job)
            .await
            .map_err(|_| ApiError::storage())?;
        let mut sequence = previous.len() as u64;
        for next in statuses {
            sequence += 1;
            repositories
                .ansible_jobs
                .append_event(&JobEvent {
                    sequence,
                    job_id: job.id,
                    event: JobEventKind::StatusChanged { status: next },
                    created_at: Utc::now(),
                })
                .await
                .map_err(|_| ApiError::storage())?;
        }
        if matches!(
            status,
            AnsibleJobStatus::Failed | AnsibleJobStatus::ReconcileRequired
        ) {
            sequence += 1;
            repositories
                .ansible_jobs
                .append_event(&JobEvent {
                    sequence,
                    job_id: job.id,
                    event: JobEventKind::Failed {
                        code: lxcup_ansible::JobFailureCode::PlaybookFailed,
                    },
                    created_at: Utc::now(),
                })
                .await
                .map_err(|_| ApiError::storage())?;
        }
        if status == AnsibleJobStatus::ReconcileRequired {
            sequence += 1;
            repositories
                .ansible_jobs
                .append_event(&JobEvent {
                    sequence,
                    job_id: job.id,
                    event: JobEventKind::ReconcileRequired,
                    created_at: Utc::now(),
                })
                .await
                .map_err(|_| ApiError::storage())?;
        }
        sequence += 1;
        repositories
            .ansible_jobs
            .append_event(&JobEvent {
                sequence,
                job_id: job.id,
                event: JobEventKind::WorkerLog {
                    source: "windows_agent".to_owned(),
                    message,
                },
                created_at: Utc::now(),
            })
            .await
            .map_err(|_| ApiError::storage())?;
    } else {
        let mut coordinator = state.ansible.write().await;
        if matches!(
            status,
            AnsibleJobStatus::Failed | AnsibleJobStatus::ReconcileRequired
        ) {
            coordinator
                .record_event(
                    job.id,
                    JobEventKind::Failed {
                        code: lxcup_ansible::JobFailureCode::PlaybookFailed,
                    },
                )
                .map_err(|_| {
                    ApiError::conflict("invalid_workflow_state", "workflow cannot accept a result")
                })?;
        }
        coordinator
            .record_event(
                job.id,
                JobEventKind::WorkerLog {
                    source: "windows_agent".to_owned(),
                    message,
                },
            )
            .map_err(|_| {
                ApiError::conflict("invalid_workflow_state", "workflow cannot accept a result")
            })?;
        for next in statuses {
            coordinator.transition(job.id, next).map_err(|_| {
                ApiError::conflict("invalid_workflow_state", "workflow cannot be completed")
            })?;
        }
    }
    Ok(())
}

fn status_name(status: AnsibleJobStatus) -> &'static str {
    match status {
        AnsibleJobStatus::Succeeded => "succeeded",
        AnsibleJobStatus::Failed => "failed",
        AnsibleJobStatus::ReconcileRequired => "reconcile_required",
        AnsibleJobStatus::Planned => "planned",
        _ => "checking",
    }
}

#[cfg(test)]
#[path = "windows_agent_workflows_tests.rs"]
mod tests;
