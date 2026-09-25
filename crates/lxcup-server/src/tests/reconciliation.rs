use super::*;
use lxcup_core::{ResourceLifecycle, ResourceTarget};

async fn unresolved_apply(state: &ApiState) -> lxcup_core::AnsibleJobId {
    let target = Target::new(
        "uncertain-apply-target",
        TargetKind::LinuxServer,
        "192.0.2.191",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential = target.credential_secret_ref;
    let agent = target.agent_secret_ref;
    state.store.write().await.targets.push(target);
    let created = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::RepairAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::RepairAgent,
            secret_refs: vec![credential, agent],
            idempotency_key: "test-uncertain-apply".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(source) = created else {
        panic!("source job should be created")
    };
    for status in [
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::ReconcileRequired,
    ] {
        state
            .ansible
            .write()
            .await
            .transition(source.id, status)
            .unwrap();
    }
    source.id
}

#[tokio::test]
async fn reconcile_endpoint_creates_one_linked_job_only_for_unclear_apply() {
    let state = ApiState::new();
    let source_id = unresolved_apply(&state).await;
    let uri = format!("/api/v1/ansible/jobs/{}/reconcile", source_id.as_uuid());

    let created = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let reconciliation_id = payload["data"]["id"].as_str().unwrap();
    assert_eq!(payload["data"]["mode"], "reconcile");
    assert_eq!(
        payload["data"]["reconciles_job_id"],
        source_id.as_uuid().to_string()
    );

    let repeated = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::OK);
    let repeated_payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(repeated.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated_payload["data"]["id"], reconciliation_id);
    assert_eq!(state.ansible.read().await.jobs().len(), 2);

    state
        .ansible
        .write()
        .await
        .transition(source_id, AnsibleJobStatus::Succeeded)
        .unwrap();
    let resolved = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/ansible/jobs/{}/reconcile",
                    source_id.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reconcile_endpoint_rejects_new_or_resolved_jobs() {
    let state = ApiState::new();
    unresolved_apply(&state).await;
    let target = Target::new(
        "new-apply-target",
        TargetKind::LinuxServer,
        "192.0.2.192",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    let target_id = target.id;
    let credential = target.credential_secret_ref;
    let agent = target.agent_secret_ref;
    state.store.write().await.targets.push(target);
    let rejected_source = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::RepairAgent,
            target: ResourceTarget::Target(target_id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Apply,
            parameters: AnsibleParameters::RepairAgent,
            secret_refs: vec![credential, agent],
            idempotency_key: "new-apply".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Operator,
        })
        .unwrap();
    let JobSubmission::Created(rejected_source) = rejected_source else {
        panic!("second source job should be created")
    };
    let uri = format!(
        "/api/v1/ansible/jobs/{}/reconcile",
        rejected_source.id.as_uuid()
    );
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.ansible.read().await.jobs().len(), 2);
}
