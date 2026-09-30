use super::{
    AnsibleJob, AnsibleJobStatus, ApiEnvelope, ApiError, ApiEvent, ApiState, ContainerId,
    CreateEnrollmentRequest, Enrollment, EnrollmentDto, EnrollmentId, EnrollmentState, Json,
    JsonBody, Path, ResourceTarget, State, StatusCode, TargetState, ensure_deployment_followups,
    envelope, parse_uuid, queue_enrollment_job,
};

pub(crate) async fn create_enrollment(
    State(state): State<ApiState>,
    JsonBody(request): JsonBody<CreateEnrollmentRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<EnrollmentDto>>), ApiError> {
    let container_id = ContainerId::new(request.container_id);
    if request.container_id == 0 {
        return Err(ApiError::bad_request(
            "invalid_container_id",
            "container id must be greater than zero",
        ));
    }

    let mut store = state.store.write().await;
    if !store
        .containers
        .iter()
        .any(|container| container.id == container_id)
    {
        return Err(ApiError::not_found("container not found"));
    }
    if let Some(target_id) = request.target_id {
        let Some(target) = store.targets.iter().find(|target| target.id == target_id) else {
            return Err(ApiError::not_found("target not found"));
        };
        if target.kind != lxcup_core::TargetKind::Lxc {
            return Err(ApiError::bad_request(
                "invalid_enrollment_target",
                "an LXC enrollment can only be linked to an LXC target",
            ));
        }
        if store.enrollments.iter().any(|enrollment| {
            enrollment.container_id != container_id && enrollment.target_id == Some(target_id)
        }) {
            return Err(ApiError::conflict(
                "target_already_enrolled",
                "this target is already linked to another LXC container",
            ));
        }
    }

    if let Some(existing_id) = store.enrollment_keys.get(&request.idempotency_key).copied() {
        let existing = store
            .enrollments
            .iter()
            .find(|enrollment| enrollment.id == existing_id)
            .expect("enrollment idempotency index must point to an enrollment");
        if existing.container_id != container_id || existing.target_id != request.target_id {
            return Err(ApiError::conflict(
                "idempotency_key_reused",
                "idempotency key is already used for another container",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(envelope(EnrollmentDto::from(existing))),
        ));
    }

    if store
        .enrollments
        .iter()
        .any(|enrollment| enrollment.container_id == container_id && enrollment.state.is_active())
    {
        return Err(ApiError::conflict(
            "enrollment_in_progress",
            "an enrollment is already in progress for this container",
        ));
    }

    let mut enrollment =
        Enrollment::new(container_id, request.idempotency_key.clone()).map_err(|_| {
            ApiError::bad_request("invalid_idempotency_key", "idempotency key is invalid")
        })?;
    enrollment.target_id = request.target_id;
    let dto = EnrollmentDto::from(&enrollment);
    store
        .enrollment_keys
        .insert(request.idempotency_key.clone(), enrollment.id);
    store.enrollments.push(enrollment);
    drop(store);

    if request.start_onboarding {
        queue_enrollment_job(&state, &request, &dto).await?;
    }

    state.publish(ApiEvent::status(
        "enrollment",
        dto.id.as_uuid().to_string(),
        "requested",
    ));
    Ok((StatusCode::ACCEPTED, Json(envelope(dto))))
}

pub(crate) async fn get_enrollment(
    State(state): State<ApiState>,
    Path(enrollment_id): Path<String>,
) -> Result<Json<ApiEnvelope<EnrollmentDto>>, ApiError> {
    let enrollment_id = EnrollmentId::from_uuid(parse_uuid(&enrollment_id, "enrollment id")?);
    let enrollment_exists = state
        .store
        .read()
        .await
        .enrollments
        .iter()
        .any(|enrollment| enrollment.id == enrollment_id);
    if !enrollment_exists {
        return Err(ApiError::not_found("enrollment not found"));
    }

    // Reconcile the enrollment view with the deployment job. This keeps the
    // onboarding page from waiting forever when the worker has already
    // finished (or rejected) the linked agent deployment.
    let job_key = format!("enrollment-{}", enrollment_id.as_uuid());
    let jobs = if let Some(repositories) = state.repositories.clone() {
        repositories
            .ansible_jobs
            .list()
            .await
            .map_err(|_| ApiError::storage())?
    } else {
        state.ansible.read().await.jobs()
    };
    if let Some(job) = jobs.iter().find(|job| job.idempotency_key == job_key) {
        let target_connected = if let ResourceTarget::Target(target_id) = job.target {
            state
                .store
                .read()
                .await
                .targets
                .iter()
                .any(|target| target.id == target_id && target.state == TargetState::Managed)
        } else {
            false
        };
        let mut store = state.store.write().await;
        if let Some(current) = store
            .enrollments
            .iter_mut()
            .find(|item| item.id == enrollment_id)
        {
            match job.status {
                AnsibleJobStatus::Failed => current
                    .fail("Agent-Deployment fehlgeschlagen. Details stehen im Workflow-Protokoll."),
                AnsibleJobStatus::Applying | AnsibleJobStatus::Succeeded
                    if current.state == EnrollmentState::Discovering =>
                {
                    let _ = current.transition_to(EnrollmentState::InstallingAgent);
                }
                AnsibleJobStatus::Succeeded if target_connected => {
                    let _ = current.transition_to(EnrollmentState::RegisteringAgent);
                    let _ = current.transition_to(EnrollmentState::Connected);
                }
                _ => {}
            }
        }
        drop(store);
        ensure_enrollment_followups(&state, enrollment_id, job).await?;
    }
    let store = state.store.read().await;
    let enrollment = store
        .enrollments
        .iter()
        .find(|enrollment| enrollment.id == enrollment_id)
        .ok_or_else(|| ApiError::not_found("enrollment not found"))?;
    Ok(Json(envelope(EnrollmentDto::from(enrollment))))
}

/// The controller owns the onboarding sequence so a browser reload or a
/// stopped UI cannot prevent the healthcheck and first inventory from being
/// queued. Idempotency keys make this safe to call on every enrollment poll.
async fn ensure_enrollment_followups(
    state: &ApiState,
    enrollment_id: EnrollmentId,
    deployment: &AnsibleJob,
) -> Result<(), ApiError> {
    ensure_deployment_followups(
        state,
        deployment,
        format!("enrollment-health-{}", enrollment_id.as_uuid()),
        format!("enrollment-packages-{}", enrollment_id.as_uuid()),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiStore;
    use lxcup_core::{
        Container, ContainerStatus, NodeId, OperatingSystem, Target, TargetId, TargetKind,
        TargetTransport,
    };

    fn container(id: u64) -> lxcup_core::Container {
        Container::new(
            ContainerId::new(id),
            NodeId::new(),
            format!("container-{id}"),
            OperatingSystem::Debian,
            ContainerStatus::Running,
        )
        .unwrap()
    }

    fn request(container_id: u64, idempotency_key: &str) -> CreateEnrollmentRequest {
        CreateEnrollmentRequest {
            container_id,
            target_id: None,
            idempotency_key: idempotency_key.to_owned(),
            start_onboarding: false,
        }
    }

    #[tokio::test]
    async fn enrollment_rejects_invalid_container_target_and_idempotency_combinations() {
        let state = ApiState::new();
        assert_eq!(
            create_enrollment(State(state.clone()), Json(request(0, "valid-key")))
                .await
                .unwrap_err()
                .code,
            "invalid_container_id"
        );
        assert_eq!(
            create_enrollment(State(state.clone()), Json(request(100, "valid-key")))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        let containers = vec![container(101), container(102)];
        let linux_target = Target::new(
            "not-an-lxc",
            TargetKind::LinuxServer,
            "192.0.2.252",
            TargetTransport::Ssh,
            lxcup_core::SecretId::new(),
            lxcup_core::SecretId::new(),
        )
        .unwrap();
        let wrong_kind_id = linux_target.id;
        let lxc_target = Target::new(
            "linked-lxc",
            TargetKind::Lxc,
            "192.0.2.253",
            TargetTransport::Ssh,
            lxcup_core::SecretId::new(),
            lxcup_core::SecretId::new(),
        )
        .unwrap();
        let lxc_target_id = lxc_target.id;
        let mut linked =
            Enrollment::new(ContainerId::new(102), "existing-link".to_owned()).unwrap();
        linked.target_id = Some(lxc_target_id);
        let store = ApiStore {
            containers,
            targets: vec![linux_target, lxc_target],
            enrollments: vec![linked],
            ..ApiStore::default()
        };
        let state = ApiState::new();
        *state.store.write().await = store;

        let mut wrong_kind = request(101, "wrong-kind");
        wrong_kind.target_id = Some(wrong_kind_id);
        assert_eq!(
            create_enrollment(State(state.clone()), Json(wrong_kind))
                .await
                .unwrap_err()
                .code,
            "invalid_enrollment_target"
        );
        let mut missing_target = request(101, "missing-target");
        missing_target.target_id = Some(TargetId::new());
        assert_eq!(
            create_enrollment(State(state.clone()), Json(missing_target))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        let mut target_in_use = request(101, "target-in-use");
        target_in_use.target_id = Some(lxc_target_id);
        assert_eq!(
            create_enrollment(State(state.clone()), Json(target_in_use))
                .await
                .unwrap_err()
                .code,
            "target_already_enrolled"
        );
        assert_eq!(
            create_enrollment(State(state.clone()), Json(request(101, "")))
                .await
                .unwrap_err()
                .code,
            "invalid_idempotency_key"
        );

        let mut active =
            Enrollment::new(ContainerId::new(101), "already-active".to_owned()).unwrap();
        active.target_id = None;
        state.store.write().await.enrollments.push(active);
        assert_eq!(
            create_enrollment(State(state.clone()), Json(request(101, "second-key")))
                .await
                .unwrap_err()
                .code,
            "enrollment_in_progress"
        );
        assert_eq!(
            get_enrollment(State(state.clone()), Path("invalid".to_owned()))
                .await
                .unwrap_err()
                .code,
            "invalid_id"
        );
        assert_eq!(
            get_enrollment(State(state), Path(uuid::Uuid::new_v4().to_string()))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }
}
