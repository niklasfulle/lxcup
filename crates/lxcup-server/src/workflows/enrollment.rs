use super::*;

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

    if let Some(existing_id) = store.enrollment_keys.get(&request.idempotency_key).copied() {
        let existing = store
            .enrollments
            .iter()
            .find(|enrollment| enrollment.id == existing_id)
            .expect("enrollment idempotency index must point to an enrollment");
        if existing.container_id != container_id {
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

    let enrollment =
        Enrollment::new(container_id, request.idempotency_key.clone()).map_err(|_| {
            ApiError::bad_request("invalid_idempotency_key", "idempotency key is invalid")
        })?;
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
        .find(|enrollment| enrollment.id == enrollment_id)
        .is_some();
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
