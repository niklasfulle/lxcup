use super::*;

/// Reconciles persisted successful deployments so onboarding can continue
/// after a browser reload or controller restart. The idempotency keys are
/// stable per target/enrollment, so polling and competing controller ticks
/// cannot create duplicate follow-up jobs.
pub(crate) async fn reconcile_onboarding_jobs(state: &ApiState) -> Result<usize, ApiError> {
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };

    let mut queued = 0;
    for deployment in jobs.iter().filter(|job| {
        job.operation == AnsibleOperation::DeployAgent && job.status == AnsibleJobStatus::Succeeded
    }) {
        let (health_key, inventory_key) =
            if let Some(enrollment_id) = deployment.idempotency_key.strip_prefix("enrollment-") {
                (
                    format!("enrollment-health-{enrollment_id}"),
                    format!("enrollment-packages-{enrollment_id}"),
                )
            } else if let Some(target_id) = deployment
                .idempotency_key
                .strip_prefix("onboarding-deploy-")
            {
                (
                    format!("onboarding-health-{target_id}"),
                    format!("onboarding-inventory-{target_id}"),
                )
            } else if let Some(rotation_id) = deployment
                .idempotency_key
                .strip_prefix("agent-token-reconfiguration-")
            {
                (
                    format!("agent-token-health-{rotation_id}"),
                    format!("agent-token-inventory-{rotation_id}"),
                )
            } else {
                continue;
            };
        match ensure_deployment_followups(state, deployment, health_key, inventory_key).await {
            Ok(created) => queued += created,
            Err(error) => {
                tracing::warn!(job_id = %deployment.id.as_uuid(), error = ?error, "onboarding follow-up reconciliation failed")
            }
        }
    }
    for update in jobs.iter().filter(|job| {
        job.operation == AnsibleOperation::UpdatePackages
            && job.mode == ExecutionMode::Apply
            && job.status == AnsibleJobStatus::Succeeded
    }) {
        match queue_package_inventory_followup(state, update).await {
            Ok(created) => queued += created,
            Err(error) => {
                tracing::warn!(job_id = %update.id.as_uuid(), error = ?error, "package update inventory follow-up reconciliation failed")
            }
        }
    }
    Ok(queued)
}

pub(super) async fn queue_package_inventory_followup(
    state: &ApiState,
    update: &AnsibleJob,
) -> Result<usize, ApiError> {
    let ResourceTarget::Target(target_id) = update.target else {
        return Ok(0);
    };
    let key = format!("package-update-inventory-{}", update.id.as_uuid());
    if find_existing_job(state, update.target, &key)
        .await?
        .is_some()
    {
        return Ok(0);
    }
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|item| item.id == target_id && item.state == TargetState::Managed)
        .cloned();
    let Some(target) = target else { return Ok(0) };
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::CollectPackageInventory,
            target: update.target,
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::CollectPackageInventory,
            secret_refs: vec![target.credential_secret_ref],
            idempotency_key: key,
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .map_err(map_ansible_error)?;
    if let JobSubmission::Created(job) = submission {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
        return Ok(1);
    }
    Ok(0)
}

pub(crate) async fn queue_agent_reconfiguration(
    state: &ApiState,
    target: &Target,
    idempotency_key: String,
) -> Result<Option<AnsibleJob>, ApiError> {
    if target.state == TargetState::Disabled {
        return Ok(None);
    }
    let resource_target = ResourceTarget::Target(target.id);
    if let Some(existing) = find_existing_job(state, resource_target, &idempotency_key).await? {
        return Ok(Some(existing));
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: resource_target,
            lifecycle: ResourceLifecycle::Pending,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
            idempotency_key,
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .map_err(map_ansible_error)?;
    let (job, created) = match submission {
        JobSubmission::Created(job) => (job, true),
        JobSubmission::Duplicate(job) => (job, false),
    };
    if created {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
    }
    Ok(Some(job))
}

pub(super) async fn ensure_deployment_followups(
    state: &ApiState,
    deployment: &AnsibleJob,
    health_key: String,
    inventory_key: String,
) -> Result<usize, ApiError> {
    if deployment.status != AnsibleJobStatus::Succeeded {
        return Ok(0);
    }
    let ResourceTarget::Target(target_id) = deployment.target else {
        return Ok(0);
    };
    let target = state
        .store
        .read()
        .await
        .targets
        .iter()
        .find(|target| target.id == target_id && target.state == TargetState::Managed)
        .cloned();
    let Some(target) = target else { return Ok(0) };
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    let health = jobs
        .iter()
        .find(|job| job.idempotency_key == health_key)
        .cloned();
    let mut created = 0;
    if health.is_none() {
        let health = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::HealthCheck,
                target: ResourceTarget::Target(target_id),
                lifecycle: ResourceLifecycle::Managed,
                mode: ExecutionMode::Check,
                parameters: AnsibleParameters::HealthCheck,
                secret_refs: Vec::new(),
                idempotency_key: health_key,
                confirmed: true,
                actor_role: ActorRole::Operator,
            })
            .map_err(map_ansible_error)?;
        if let JobSubmission::Created(job) = health {
            persist_created_job(state, &job).await?;
            state.publish(ApiEvent::status(
                "ansible_job",
                job.id.as_uuid().to_string(),
                "queued",
            ));
            created += 1;
        }
        return Ok(created);
    }
    if health
        .as_ref()
        .is_some_and(|job| job.status != AnsibleJobStatus::Succeeded)
    {
        return Ok(created);
    }
    if jobs.iter().any(|job| job.idempotency_key == inventory_key) {
        return Ok(created);
    }
    let submission = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::CollectPackageInventory,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::CollectPackageInventory,
            secret_refs: vec![target.credential_secret_ref],
            idempotency_key: inventory_key,
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .map_err(map_ansible_error)?;
    if let JobSubmission::Created(job) = submission {
        persist_created_job(state, &job).await?;
        state.publish(ApiEvent::status(
            "ansible_job",
            job.id.as_uuid().to_string(),
            "queued",
        ));
        created += 1;
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{TargetKind, TargetTransport};

    fn managed_target(name: &str) -> Target {
        let mut target = Target::new(
            name,
            TargetKind::LinuxServer,
            "192.0.2.111",
            TargetTransport::Ssh,
            SecretId::new(),
            SecretId::new(),
        )
        .unwrap();
        target.mark_managed();
        target
    }

    async fn completed_deployment(
        state: &ApiState,
        target: &Target,
        idempotency_key: &str,
    ) -> AnsibleJob {
        state.store.write().await.targets.push(target.clone());
        let submission = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::DeployAgent,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Pending,
                mode: ExecutionMode::Apply,
                parameters: AnsibleParameters::DeployAgent {
                    agent_version: "0.2.0".to_owned(),
                },
                secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
                idempotency_key: idempotency_key.to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap();
        let JobSubmission::Created(job) = submission else {
            panic!("deployment should be created")
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
        coordinator.job(job.id).unwrap()
    }

    #[tokio::test]
    async fn deployment_followups_wait_for_managed_target_and_successful_healthcheck() {
        let state = ApiState::new();
        let target = managed_target("reconciliation-target");
        let deployment = completed_deployment(&state, &target, "onboarding-deploy-first").await;
        let health_key = "onboarding-health-first";
        let inventory_key = "onboarding-inventory-first";

        assert_eq!(
            ensure_deployment_followups(
                &state,
                &deployment,
                health_key.to_owned(),
                inventory_key.to_owned(),
            )
            .await
            .unwrap(),
            1
        );
        let health = state
            .ansible
            .read()
            .await
            .jobs()
            .into_iter()
            .find(|job| job.idempotency_key == health_key)
            .unwrap();
        assert_eq!(
            ensure_deployment_followups(
                &state,
                &deployment,
                health_key.to_owned(),
                inventory_key.to_owned(),
            )
            .await
            .unwrap(),
            0
        );

        let mut coordinator = state.ansible.write().await;
        coordinator
            .transition(health.id, AnsibleJobStatus::Checking)
            .unwrap();
        coordinator
            .transition(health.id, AnsibleJobStatus::Planned)
            .unwrap();
        coordinator
            .transition(health.id, AnsibleJobStatus::Applying)
            .unwrap();
        coordinator
            .transition(health.id, AnsibleJobStatus::Succeeded)
            .unwrap();
        drop(coordinator);
        assert_eq!(
            ensure_deployment_followups(
                &state,
                &deployment,
                health_key.to_owned(),
                inventory_key.to_owned(),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            ensure_deployment_followups(
                &state,
                &deployment,
                health_key.to_owned(),
                inventory_key.to_owned(),
            )
            .await
            .unwrap(),
            0
        );
        assert!(state.ansible.read().await.jobs().iter().any(|job| {
            job.idempotency_key == inventory_key
                && job.operation == AnsibleOperation::CollectPackageInventory
        }));
    }

    #[tokio::test]
    async fn reconfiguration_is_disabled_safe_and_idempotent() {
        let state = ApiState::new();
        let mut disabled = managed_target("disabled-reconfigure-target");
        disabled.state = TargetState::Disabled;
        assert!(
            queue_agent_reconfiguration(&state, &disabled, "disabled-key".to_owned())
                .await
                .unwrap()
                .is_none()
        );

        let target = managed_target("reconfigure-target");
        state.store.write().await.targets.push(target.clone());
        let first = queue_agent_reconfiguration(
            &state,
            &target,
            "agent-token-reconfiguration-rotation-1".to_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.operation, AnsibleOperation::DeployAgent);
        assert_eq!(first.mode, ExecutionMode::Apply);
        let repeated = queue_agent_reconfiguration(
            &state,
            &target,
            "agent-token-reconfiguration-rotation-1".to_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(repeated.id, first.id);
        assert_eq!(state.ansible.read().await.jobs().len(), 1);
    }

    #[tokio::test]
    async fn reconciliation_ignores_incomplete_and_unrecognized_deployments() {
        let state = ApiState::new();
        let target = managed_target("ignored-deployment-target");
        state.store.write().await.targets.push(target.clone());
        let queued = state
            .ansible
            .write()
            .await
            .submit(AnsibleJobRequest {
                operation: AnsibleOperation::DeployAgent,
                target: ResourceTarget::Target(target.id),
                lifecycle: ResourceLifecycle::Pending,
                mode: ExecutionMode::Apply,
                parameters: AnsibleParameters::DeployAgent {
                    agent_version: "0.2.0".to_owned(),
                },
                secret_refs: vec![target.credential_secret_ref, target.agent_secret_ref],
                idempotency_key: "unrecognized-deployment-key".to_owned(),
                confirmed: true,
                actor_role: ActorRole::Admin,
            })
            .unwrap();
        assert!(matches!(queued, JobSubmission::Created(_)));
        assert_eq!(reconcile_onboarding_jobs(&state).await.unwrap(), 0);
        assert_eq!(state.ansible.read().await.jobs().len(), 1);
    }
}
