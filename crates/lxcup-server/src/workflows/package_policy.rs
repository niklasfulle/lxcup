use super::{
    AnsibleJob, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, ApiError, ApiState,
    ContainerId, ContainerManagementState, CreateAnsibleJobRequest, ExecutionMode,
    ResourceLifecycle, ResourceTarget, SecretId, TargetId, TargetState, UpdateRisk,
    configured_ansible_secret_refs, map_ansible_error,
};

pub(super) async fn validate_package_update(
    state: &ApiState,
    request: &CreateAnsibleJobRequest,
    target: ResourceTarget,
) -> Result<(), ApiError> {
    if request.operation != AnsibleOperation::UpdatePackages {
        return Ok(());
    }
    let ResourceTarget::Target(target_id) = target else {
        return Err(ApiError::bad_request(
            "target_policy_required",
            "package updates require a registered target and update policy",
        ));
    };
    let policy_id = request.policy_id.as_deref().ok_or_else(|| {
        ApiError::bad_request(
            "update_policy_required",
            "package updates require an explicit update policy",
        )
    })?;
    let policy = load_update_policy(state, target_id, policy_id).await?;
    let packages = update_packages(request)?;
    validate_policy_allows_packages(&policy, target_id, packages)?;

    match request.mode {
        ExecutionMode::Plan => validate_plan_key(request, policy_id),
        ExecutionMode::Apply => {
            validate_package_apply(
                state, request, target, target_id, policy_id, &policy, packages,
            )
            .await
        }
        _ => Err(ApiError::bad_request(
            "package_plan_mode_required",
            "package updates must first be planned; only a matching successful plan may be applied",
        )),
    }
}

async fn load_update_policy(
    state: &ApiState,
    target_id: TargetId,
    policy_id: &str,
) -> Result<lxcup_core::UpdatePolicy, ApiError> {
    let store = state.store.read().await;
    let target_kind = store
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .map(|target| target.kind)
        .ok_or_else(|| ApiError::not_found("target not found"))?;
    if target_kind == lxcup_core::TargetKind::WindowsServer {
        return Err(ApiError::bad_request(
            "package_update_platform_unsupported",
            "package-name update plans are currently supported on Debian and Ubuntu targets only",
        ));
    }
    store
        .update_policies
        .iter()
        .find(|policy| policy.id == policy_id && policy.enabled)
        .cloned()
        .ok_or_else(|| {
            ApiError::bad_request(
                "update_policy_unavailable",
                "the selected update policy is missing or disabled",
            )
        })
}

fn update_packages(request: &CreateAnsibleJobRequest) -> Result<&[String], ApiError> {
    let AnsibleParameters::UpdatePackages { packages } = &request.parameters else {
        return Err(ApiError::bad_request(
            "invalid_package_plan",
            "package update parameters are invalid",
        ));
    };
    Ok(packages)
}

fn validate_policy_allows_packages(
    policy: &lxcup_core::UpdatePolicy,
    target_id: TargetId,
    packages: &[String],
) -> Result<(), ApiError> {
    let denied_target_or_risk =
        !policy.allowed_targets.contains(&target_id) || policy.maximum_risk < UpdateRisk::High;
    let requests_all_packages = packages == ["*"];
    let denied_package = (requests_all_packages && !policy.allowed_packages.is_empty())
        || (!policy.allowed_packages.is_empty()
            && packages
                .iter()
                .any(|package| !policy.allowed_packages.contains(package)));
    if denied_target_or_risk || denied_package {
        return Err(ApiError::forbidden(
            "update_policy_denied",
            "target, package, risk, or maintenance window is not allowed by the selected policy",
        ));
    }
    Ok(())
}

fn validate_plan_key(request: &CreateAnsibleJobRequest, policy_id: &str) -> Result<(), ApiError> {
    if request.approved_plan_job_id.is_none()
        && request
            .idempotency_key
            .starts_with(&format!("package-plan:{policy_id}:"))
    {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "package_plan_mode_required",
            "package updates must first be planned; only a matching successful plan may be applied",
        ))
    }
}

async fn validate_package_apply(
    state: &ApiState,
    request: &CreateAnsibleJobRequest,
    target: ResourceTarget,
    target_id: TargetId,
    policy_id: &str,
    policy: &lxcup_core::UpdatePolicy,
    packages: &[String],
) -> Result<(), ApiError> {
    let plan_id = request.approved_plan_job_id.ok_or_else(|| {
        ApiError::bad_request(
            "package_plan_required",
            "apply requires a completed package plan",
        )
    })?;
    validate_apply_key(request, plan_id)?;
    validate_apply_window(policy, target_id, packages)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "ansible_confirmation_required",
            "applying a package plan requires explicit confirmation",
        ));
    }
    let plan = load_package_plan(state, plan_id).await?;
    if !plan_matches_request(&plan, request, target, policy_id) {
        return Err(ApiError::conflict(
            "package_plan_mismatch",
            "the approved plan must be successful and match the exact target and package list",
        ));
    }
    reject_reused_plan(state, plan_id).await
}

fn validate_apply_key(
    request: &CreateAnsibleJobRequest,
    plan_id: lxcup_core::AnsibleJobId,
) -> Result<(), ApiError> {
    if request
        .idempotency_key
        .starts_with(&format!("package-apply:{}:", plan_id.as_uuid()))
    {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "invalid_package_apply_key",
            "the apply request must be tied to its approved plan",
        ))
    }
}

fn validate_apply_window(
    policy: &lxcup_core::UpdatePolicy,
    target_id: TargetId,
    packages: &[String],
) -> Result<(), ApiError> {
    if packages
        .iter()
        .any(|package| !policy.allows(target_id, package, UpdateRisk::High, chrono::Utc::now()))
    {
        return Err(ApiError::forbidden(
            "update_policy_window_closed",
            "package apply is only allowed inside the policy maintenance window",
        ));
    }
    Ok(())
}

async fn load_package_plan(
    state: &ApiState,
    plan_id: lxcup_core::AnsibleJobId,
) -> Result<AnsibleJob, ApiError> {
    if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .ansible_jobs
            .find_by_id(plan_id)
            .await
            .map_err(|_| ApiError::storage())?
            .ok_or_else(|| ApiError::not_found("package plan not found"))
    } else {
        state
            .ansible
            .read()
            .await
            .job(plan_id)
            .map_err(map_ansible_error)
    }
}

fn plan_matches_request(
    plan: &AnsibleJob,
    request: &CreateAnsibleJobRequest,
    target: ResourceTarget,
    policy_id: &str,
) -> bool {
    plan.operation == AnsibleOperation::UpdatePackages
        && plan.mode == ExecutionMode::Plan
        && plan.status == AnsibleJobStatus::Succeeded
        && plan.target == target
        && plan.parameters == request.parameters
        && plan
            .idempotency_key
            .starts_with(&format!("package-plan:{policy_id}:"))
}

async fn reject_reused_plan(
    state: &ApiState,
    plan_id: lxcup_core::AnsibleJobId,
) -> Result<(), ApiError> {
    let existing_jobs = if let Some(repositories) = state.repositories.as_ref() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    if existing_jobs.iter().any(|job| {
        job.operation == AnsibleOperation::UpdatePackages
            && job.mode == ExecutionMode::Apply
            && job
                .idempotency_key
                .starts_with(&format!("package-apply:{}:", plan_id.as_uuid()))
    }) {
        return Err(ApiError::conflict(
            "package_plan_already_used",
            "this plan already has an apply attempt; create a fresh plan before resuming",
        ));
    }
    Ok(())
}

pub(super) async fn resolve_ansible_target(
    state: &ApiState,
    request: &CreateAnsibleJobRequest,
) -> Result<(ResourceTarget, ResourceLifecycle, Option<SecretId>), ApiError> {
    if let Some(target_id) = request.target_id {
        let target = state
            .store
            .read()
            .await
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("target not found"))?;
        let lifecycle = match target.state {
            TargetState::Pending => ResourceLifecycle::Pending,
            TargetState::Managed => ResourceLifecycle::Managed,
            TargetState::Disabled => ResourceLifecycle::Disabled,
        };
        return Ok((
            ResourceTarget::Target(target_id),
            lifecycle,
            Some(target.credential_secret_ref),
        ));
    }
    if request.container_id == 0 {
        return Err(ApiError::bad_request(
            "target_required",
            "target id must be provided",
        ));
    }
    let container_id = ContainerId::new(request.container_id);
    let lifecycle = state
        .store
        .read()
        .await
        .containers
        .iter()
        .find(|container| container.id == container_id)
        .map(|container| match container.management_state {
            ContainerManagementState::Discovered => ResourceLifecycle::Discovered,
            ContainerManagementState::Managed => ResourceLifecycle::Managed,
            ContainerManagementState::Ignored | ContainerManagementState::Disabled => {
                ResourceLifecycle::Disabled
            }
        })
        .ok_or_else(|| ApiError::not_found("container not found"))?;
    Ok((ResourceTarget::Container(container_id), lifecycle, None))
}

pub(super) fn resolve_job_secret_refs(
    operation: AnsibleOperation,
    target_secret_ref: Option<SecretId>,
) -> Result<Vec<SecretId>, ApiError> {
    if operation == AnsibleOperation::HealthCheck {
        return Ok(Vec::new());
    }
    target_secret_ref.map_or_else(configured_ansible_secret_refs, |secret| Ok(vec![secret]))
}

pub(super) async fn find_existing_job(
    state: &ApiState,
    target: ResourceTarget,
    idempotency_key: &str,
) -> Result<Option<AnsibleJob>, ApiError> {
    let Some(repositories) = state.repositories.clone() else {
        return Ok(None);
    };
    if let Some(existing) = repositories
        .ansible_jobs
        .find_by_idempotency_key(target, idempotency_key)
        .await
        .map_err(|_| ApiError::storage())?
    {
        return Ok(Some(existing));
    }
    if repositories
        .ansible_jobs
        .has_active_target(target)
        .await
        .map_err(|_| ApiError::storage())?
    {
        return Err(ApiError::conflict(
            "ansible_target_busy",
            "another job is active for this target",
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod package_policy_tests {
    use super::super::reconcile_onboarding_jobs;
    use super::*;
    use lxcup_ansible::{AnsibleJobRequest, JobSubmission};
    use lxcup_core::{ActorRole, Target, TargetId, TargetKind, TargetTransport, UpdatePolicy};
    use uuid::Uuid;

    fn package_request(
        mode: ExecutionMode,
        approved_plan_job_id: Option<lxcup_core::AnsibleJobId>,
        packages: &[&str],
    ) -> CreateAnsibleJobRequest {
        let idempotency_key = match (mode, approved_plan_job_id) {
            (ExecutionMode::Plan, _) => {
                format!("package-plan:security:{}", Uuid::new_v4())
            }
            (ExecutionMode::Apply, Some(plan_id)) => {
                format!("package-apply:{}:{}", plan_id.as_uuid(), Uuid::new_v4())
            }
            _ => Uuid::new_v4().to_string(),
        };
        CreateAnsibleJobRequest {
            operation: AnsibleOperation::UpdatePackages,
            target_id: Some(TargetId::new()),
            container_id: 0,
            mode,
            parameters: AnsibleParameters::UpdatePackages {
                packages: packages.iter().map(|item| (*item).to_owned()).collect(),
            },
            idempotency_key,
            confirmed: mode == ExecutionMode::Apply,
            policy_id: Some("security".to_owned()),
            approved_plan_job_id,
        }
    }

    #[tokio::test]
    async fn package_apply_requires_policy_and_a_matching_successful_plan() {
        let state = ApiState::new();
        let mut target = Target::new(
            "policy-target",
            TargetKind::LinuxServer,
            "192.0.2.1",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        {
            let mut store = state.store.write().await;
            store.update_policies.push(UpdatePolicy {
                id: "security".to_owned(),
                allowed_targets: vec![target.id],
                allowed_packages: vec!["curl".to_owned()],
                maintenance_start_minute: 0,
                maintenance_end_minute: 1439,
                timezone: "UTC".to_owned(),
                maximum_risk: UpdateRisk::High,
                enabled: true,
            });
            store.targets.push(target.clone());
        }
        let mut request = package_request(ExecutionMode::Plan, None, &["curl"]);
        request.target_id = Some(target.id);
        assert!(
            validate_package_update(&state, &request, ResourceTarget::Target(target.id))
                .await
                .is_ok()
        );
        request.parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["wget".to_owned()],
        };
        assert_eq!(
            validate_package_update(&state, &request, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "update_policy_denied"
        );

        let parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()],
        };
        let JobSubmission::Created(failed_plan) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Plan,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "package-plan:security:failed-plan".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("plan should be created")
        };
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(failed_plan.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(failed_plan.id, AnsibleJobStatus::Failed)
            .unwrap();
        drop(coordinator);
        let mut rejected_apply =
            package_request(ExecutionMode::Apply, Some(failed_plan.id), &["curl"]);
        rejected_apply.target_id = Some(target.id);
        assert_eq!(
            validate_package_update(&state, &rejected_apply, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "package_plan_mismatch"
        );

        let plan = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Plan,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "package-plan:security:approved-plan".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap();
        let JobSubmission::Created(plan) = plan else {
            panic!("plan should be created")
        };
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(plan.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Planned)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Applying)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Succeeded)
            .unwrap();
        drop(coordinator);
        let mut apply = package_request(ExecutionMode::Apply, Some(plan.id), &["curl"]);
        apply.target_id = Some(target.id);
        apply.confirmed = true;
        assert!(
            validate_package_update(&state, &apply, ResourceTarget::Target(target.id))
                .await
                .is_ok()
        );
        let JobSubmission::Created(applied_job) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Apply,
                parameters: parameters.clone(),
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: apply.idempotency_key.clone(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("apply job should be created")
        };
        let mut coordinator = state.ansible.write().await;
        for status in [
            AnsibleJobStatus::Checking,
            AnsibleJobStatus::Planned,
            AnsibleJobStatus::Applying,
            AnsibleJobStatus::Failed,
        ] {
            coordinator.transition(applied_job.id, status).unwrap();
        }
        drop(coordinator);
        let retry = package_request(ExecutionMode::Apply, Some(plan.id), &["curl"]);
        assert_eq!(
            validate_package_update(&state, &retry, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "package_plan_already_used"
        );
        apply.parameters = AnsibleParameters::UpdatePackages {
            packages: vec!["wget".to_owned()],
        };
        assert_eq!(
            validate_package_update(&state, &apply, ResourceTarget::Target(target.id))
                .await
                .unwrap_err()
                .code,
            "update_policy_denied"
        );
    }

    #[tokio::test]
    async fn successful_package_apply_queues_one_inventory_refresh() {
        let state = ApiState::new();
        let mut target = Target::new(
            "updated-target",
            TargetKind::LinuxServer,
            "192.0.2.9",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        state.store.write().await.targets.push(target.clone());
        let JobSubmission::Created(job) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Apply,
                parameters: AnsibleParameters::UpdatePackages {
                    packages: vec!["curl".to_owned()],
                },
                secret_refs: vec![target.credential_secret_ref],
                idempotency_key: "completed-package-update".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("package job should be created")
        };
        let mut coordinator = state.ansible.write().await;
        for status in [
            AnsibleJobStatus::Checking,
            AnsibleJobStatus::Planned,
            AnsibleJobStatus::Applying,
            AnsibleJobStatus::Succeeded,
        ] {
            coordinator.transition(job.id, status).unwrap();
        }
        drop(coordinator);

        assert_eq!(reconcile_onboarding_jobs(&state).await.unwrap(), 1);
        assert_eq!(reconcile_onboarding_jobs(&state).await.unwrap(), 0);
        let jobs = state.ansible.read().await.jobs();
        assert!(jobs.iter().any(|item| {
            item.operation == AnsibleOperation::CollectPackageInventory
                && item.idempotency_key == format!("package-update-inventory-{}", job.id.as_uuid())
        }));
    }

    #[tokio::test]
    async fn all_packages_plan_requires_a_policy_without_package_restrictions() {
        let state = ApiState::new();
        let mut target = Target::new(
            "all-updates-target",
            TargetKind::LinuxServer,
            "192.0.2.10",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        let target_id = target.id;
        let target_secret_ref = target.credential_secret_ref;
        {
            let mut store = state.store.write().await;
            store.targets.push(target.clone());
            store.update_policies.push(UpdatePolicy {
                id: "standard".to_owned(),
                allowed_targets: vec![target_id],
                allowed_packages: vec![],
                maintenance_start_minute: 0,
                maintenance_end_minute: 1439,
                timezone: "UTC".to_owned(),
                maximum_risk: UpdateRisk::High,
                enabled: true,
            });
        }
        let mut request = package_request(ExecutionMode::Plan, None, &["*"]);
        request.target_id = Some(target_id);
        request.policy_id = Some("standard".to_owned());
        request.idempotency_key = "package-plan:standard:all-updates".to_owned();
        assert!(
            validate_package_update(&state, &request, ResourceTarget::Target(target_id))
                .await
                .is_ok()
        );

        let JobSubmission::Created(plan) = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::UpdatePackages,
                target: ResourceTarget::Target(target_id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Plan,
                parameters: request.parameters.clone(),
                secret_refs: vec![target_secret_ref],
                idempotency_key: request.idempotency_key.clone(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap()
        else {
            panic!("all-updates plan should be created")
        };
        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(plan.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Planned)
            .unwrap();
        coordinator
            .transition(plan.id, AnsibleJobStatus::Succeeded)
            .unwrap();
        drop(coordinator);
        let mut apply = package_request(ExecutionMode::Apply, Some(plan.id), &["*"]);
        apply.target_id = Some(target_id);
        apply.policy_id = Some("standard".to_owned());
        assert!(
            validate_package_update(&state, &apply, ResourceTarget::Target(target_id))
                .await
                .is_ok()
        );

        state.store.write().await.update_policies[0].allowed_packages = vec!["curl".to_owned()];
        assert_eq!(
            validate_package_update(&state, &request, ResourceTarget::Target(target_id))
                .await
                .unwrap_err()
                .code,
            "update_policy_denied"
        );
    }
}
