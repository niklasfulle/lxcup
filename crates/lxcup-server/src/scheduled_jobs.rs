use super::{ApiEvent, ApiState};
use crate::workflows;
use lxcup_agent::AgentHeartbeat;
use lxcup_ansible::{
    AnsibleJobRequest, AnsibleOperation, AnsibleParameters, ExecutionMode, JobSubmission,
};
use lxcup_core::{
    ActorRole, EnrollmentState, ResourceLifecycle, ResourceTarget, SecretId, Target, TargetId,
    TargetKind, TargetState, ThresholdMetric, ThresholdRule, UpdateRisk,
};

pub(crate) async fn scheduled_target(
    state: &ApiState,
    schedule: &lxcup_core::JobSchedule,
    target_id: TargetId,
) -> Result<Target, String> {
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id && target.state == TargetState::Managed)
        .cloned()
        .ok_or_else(|| format!("target {} is not managed", target_id.as_uuid()))?;
    if let Some(repositories) = state.repositories.clone() {
        if repositories
            .ansible_jobs
            .has_active_target(ResourceTarget::Target(target_id))
            .await
            .unwrap_or(false)
        {
            return Err(format!(
                "target {} already has an active job",
                target_id.as_uuid()
            ));
        }
    }
    if let Some(rule) = schedule.threshold {
        let actual = state
            .store
            .read()
            .await
            .agent_reports
            .get(&target_id)
            .and_then(|heartbeat| threshold_value(rule, heartbeat));
        if !rule.triggered(actual) {
            let message = format!(
                "schedule {} threshold not met for target {}",
                schedule.id,
                target_id.as_uuid()
            );
            state.publish(ApiEvent::Error {
                code: "schedule_threshold_not_met".to_owned(),
                message: message.clone(),
                request_id: schedule.id.clone(),
            });
            return Err(message);
        }
    }
    Ok(target)
}

pub(crate) async fn dispatch_scheduled_target(
    state: &ApiState,
    schedule: &lxcup_core::JobSchedule,
    target_id: TargetId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    let target = scheduled_target(state, schedule, target_id).await?;
    if schedule.operation == "docker_discovery" {
        return dispatch_scheduled_docker_discovery(state, target_id, &target).await;
    }
    let request = scheduled_job_request(state, schedule, target_id, &target, now).await?;
    let submission = state.ansible.write().await.submit(request);
    match submission {
        Ok(JobSubmission::Created(job)) => {
            workflows::persist_created_job(state, &job)
                .await
                .map_err(|_| "scheduled job persistence failed".to_owned())?;
            state.publish(ApiEvent::status(
                "ansible_job",
                job.id.as_uuid().to_string(),
                "queued",
            ));
            Ok(true)
        }
        Ok(JobSubmission::Duplicate(_)) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

async fn dispatch_scheduled_docker_discovery(
    state: &ApiState,
    target_id: TargetId,
    target: &Target,
) -> Result<bool, String> {
    if target.kind != TargetKind::Lxc {
        return Err("Docker discovery schedules require an LXC target".to_owned());
    }
    let container_id = state
        .store
        .read()
        .await
        .enrollments
        .iter()
        .rev()
        .find(|enrollment| {
            enrollment.target_id == Some(target_id)
                && enrollment.state == EnrollmentState::Connected
        })
        .map(|enrollment| enrollment.container_id)
        .ok_or_else(|| "LXC target has no connected Docker discovery host".to_owned())?;
    crate::run_docker_discovery(state, container_id)
        .await
        .map(|_| true)
        .map_err(|error| format!("scheduled Docker discovery failed ({})", error.code))
}

async fn scheduled_job_request(
    state: &ApiState,
    schedule: &lxcup_core::JobSchedule,
    target_id: TargetId,
    target: &Target,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<AnsibleJobRequest, String> {
    let (operation, parameters, mode, secret_refs) =
        scheduled_operation(state, schedule, target_id, target, now).await?;
    Ok(AnsibleJobRequest {
        operation,
        target: ResourceTarget::Target(target_id),
        lifecycle: ResourceLifecycle::Managed,
        mode,
        parameters,
        secret_refs,
        idempotency_key: format!(
            "schedule-{}-{}-{}",
            schedule.id,
            target_id.as_uuid(),
            schedule.next_run_at.timestamp()
        ),
        confirmed: true,
        actor_role: ActorRole::Operator,
    })
}

async fn scheduled_operation(
    state: &ApiState,
    schedule: &lxcup_core::JobSchedule,
    target_id: TargetId,
    target: &Target,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<
    (
        AnsibleOperation,
        AnsibleParameters,
        ExecutionMode,
        Vec<SecretId>,
    ),
    String,
> {
    match schedule.operation.as_str() {
        "health_check" => Ok((
            AnsibleOperation::HealthCheck,
            AnsibleParameters::HealthCheck,
            ExecutionMode::Check,
            Vec::new(),
        )),
        "collect_package_inventory" => Ok((
            AnsibleOperation::CollectPackageInventory,
            AnsibleParameters::CollectPackageInventory,
            ExecutionMode::Check,
            vec![target.credential_secret_ref],
        )),
        "update_packages" => Ok((
            AnsibleOperation::UpdatePackages,
            scheduled_update_parameters(state, schedule, target_id, now).await?,
            ExecutionMode::Plan,
            vec![target.credential_secret_ref],
        )),
        _ => Err("schedule operation is no longer registered".to_owned()),
    }
}

async fn scheduled_update_parameters(
    state: &ApiState,
    schedule: &lxcup_core::JobSchedule,
    target_id: TargetId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<AnsibleParameters, String> {
    let policy = {
        let store = state.store.read().await;
        schedule.policy_id.as_deref().and_then(|policy_id| {
            store
                .update_policies
                .iter()
                .find(|policy| policy.id == policy_id)
                .cloned()
        })
    };
    let Some(policy) = policy else {
        return Err("scheduled package update has no valid policy".to_owned());
    };
    let packages = policy.allowed_packages.clone();
    if packages.is_empty() {
        return Err("scheduled package updates require an explicit package allowlist".to_owned());
    }
    if !policy.enabled
        || !policy.allowed_targets.contains(&target_id)
        || packages
            .iter()
            .any(|package| !policy.allows(target_id, package, UpdateRisk::High, now))
    {
        return Err("scheduled package update is outside its policy".to_owned());
    }
    Ok(AnsibleParameters::UpdatePackages { packages })
}

pub(crate) fn threshold_value(rule: ThresholdRule, heartbeat: &AgentHeartbeat) -> Option<u64> {
    let sample = heartbeat.telemetry.samples.last();
    match rule.metric {
        ThresholdMetric::CpuBasisPoints => {
            sample.and_then(|item| item.cpu_basis_points.map(u64::from))
        }
        ThresholdMetric::MemoryBasisPoints => {
            sample.and_then(|item| item.memory_basis_points.map(u64::from))
        }
        ThresholdMetric::StorageBasisPoints => {
            sample.and_then(|item| item.storage_basis_points.map(u64::from))
        }
        ThresholdMetric::HeartbeatAgeSeconds => Some(
            chrono::Utc::now()
                .signed_duration_since(heartbeat.sent_at)
                .num_seconds()
                .max(0) as u64,
        ),
    }
}
