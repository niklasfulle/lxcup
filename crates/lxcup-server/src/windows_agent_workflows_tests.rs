use super::*;
use crate::AuthConfig;
use lxcup_ansible::{AnsibleJobRequest, JobSubmission};
use lxcup_core::{ActorRole, ResourceLifecycle, SecretId, Target};
use lxcup_secrets::{CreateSecret, InMemorySecretStore, SecretStore};
use std::sync::Arc;
use uuid::Uuid;

async fn windows_agent_state() -> (ApiState, Target, String) {
    let secret_store = InMemorySecretStore::default();
    let token = "windows-agent-test-token";
    let secret = secret_store
        .create(CreateSecret {
            name: "windows-agent-test".to_owned(),
            kind: lxcup_core::SecretKind::AgentToken,
            scope: lxcup_core::SecretScope::Global,
            value: lxcup_core::SecretValue::new(token).unwrap(),
        })
        .unwrap();
    let mut target = Target::new_agent_only(
        "windows-workflow-test",
        TargetKind::WindowsServer,
        "192.0.2.50",
        secret.metadata.id,
    )
    .unwrap();
    target.mark_managed();
    let state = ApiState::new()
        .with_auth_config(AuthConfig::disabled())
        .with_secret_store(Arc::new(secret_store));
    state.store.write().await.targets.push(target.clone());
    (state, target, token.to_owned())
}

fn package_job(packages: &[&str], mode: ExecutionMode) -> AnsibleJob {
    AnsibleJob {
        id: lxcup_core::AnsibleJobId::new(),
        operation: AnsibleOperation::UpdatePackages,
        playbook: "packages/update.yml".to_owned(),
        playbook_version: "1".to_owned(),
        target: ResourceTarget::Target(TargetId::new()),
        mode,
        parameters: AnsibleParameters::UpdatePackages {
            packages: packages.iter().map(|value| (*value).to_owned()).collect(),
        },
        secret_refs: Vec::new(),
        idempotency_key: "windows-update-test".to_owned(),
        parameter_hash: "hash".to_owned(),
        status: if mode == ExecutionMode::Apply {
            AnsibleJobStatus::Applying
        } else {
            AnsibleJobStatus::Checking
        },
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn windows_apply_command_requires_exact_winget_ids() {
    let command = workflow_command(package_job(
        &["Microsoft.PowerToys", "7zip.7zip"],
        ExecutionMode::Apply,
    ))
    .unwrap();
    assert_eq!(command.action, AgentAction::Apply);
    assert_eq!(command.packages, ["Microsoft.PowerToys", "7zip.7zip"]);
    assert!(workflow_command(package_job(&["*"], ExecutionMode::Apply)).is_err());
}

#[test]
fn windows_workflow_command_maps_only_supported_operations_and_modes() {
    let mut job = package_job(&["7zip.7zip"], ExecutionMode::Plan);
    job.operation = AnsibleOperation::HealthCheck;
    job.parameters = AnsibleParameters::HealthCheck;
    assert_eq!(workflow_command(job).unwrap().action, AgentAction::Health);

    let mut job = package_job(&[], ExecutionMode::Check);
    job.operation = AnsibleOperation::CollectPackageInventory;
    job.parameters = AnsibleParameters::CollectPackageInventory;
    assert_eq!(workflow_command(job).unwrap().action, AgentAction::Scan);

    assert_eq!(
        workflow_command(package_job(&["7zip.7zip"], ExecutionMode::Reconcile))
            .unwrap()
            .action,
        AgentAction::Scan
    );
    assert!(workflow_command(package_job(&[], ExecutionMode::Plan)).is_err());

    let mut unsupported = package_job(&[], ExecutionMode::Check);
    unsupported.operation = AnsibleOperation::ConfigureTarget;
    unsupported.parameters = AnsibleParameters::ConfigureTarget {
        desired_hostname: Some("host".to_owned()),
    };
    assert_eq!(
        workflow_command(unsupported).unwrap_err().code,
        "windows_workflow_unsupported"
    );
}

#[test]
fn windows_plan_summary_is_derived_from_local_winget_listing() {
    let job = package_job(&["7zip.7zip"], ExecutionMode::Plan);
    let result = AgentWorkflowResult {
            response: lxcup_agent::AgentCommandResponse {
                request_id: Uuid::new_v4(),
                success: true,
                exit_code: 0,
                stdout: r#"[{"id":"7zip.7zip","name":"7-Zip 24.09 (x64)","version":"24.09","candidate_version":"25.01","source":"winget"}]"#.to_owned(),
                stderr: String::new(),
                reboot_required: false,
                duration_ms: 15,
            },
        };
    let (summary, usable) = result_summary(&job, &result);
    assert!(usable);
    assert!(summary.contains("7zip.7zip: 24.09 -> 25.01"));
}

#[test]
fn windows_plan_without_a_selected_update_fails_closed() {
    let job = package_job(&["7zip.7zip"], ExecutionMode::Plan);
    let result = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: true,
            exit_code: 0,
            stdout: "[]".to_owned(),
            stderr: String::new(),
            reboot_required: false,
            duration_ms: 15,
        },
    };
    let (summary, usable) = result_summary(&job, &result);
    assert!(!usable);
    assert!(summary.contains("no available updates"));
}

#[test]
fn windows_agent_result_summaries_fail_closed_for_bad_or_unselected_updates() {
    let mut job = package_job(&["7zip.7zip"], ExecutionMode::Plan);
    let mut result = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: "local scan failed".to_owned(),
            reboot_required: false,
            duration_ms: 1,
        },
    };
    assert!(!result_summary(&job, &result).1);

    result.response.success = true;
    result.response.stdout = "not a winget table".to_owned();
    assert!(
        result_summary(&job, &result)
            .0
            .contains("invalid or unsupported")
    );

    result.response.stdout = r#"[{"id":"7zip.7zip","name":"7-Zip","version":"24.09","candidate_version":"25.01","source":"winget"}]"#.to_owned();
    job.parameters = AnsibleParameters::HealthCheck;
    assert!(
        result_summary(&job, &result)
            .0
            .contains("parameters were invalid")
    );

    job.parameters = AnsibleParameters::UpdatePackages {
        packages: vec!["unselected.package".to_owned()],
    };
    assert!(
        result_summary(&job, &result)
            .0
            .contains("no available updates")
    );
}

#[test]
fn windows_inventory_failure_summary_uses_only_safe_diagnostic_codes() {
    let mut job = package_job(&[], ExecutionMode::Check);
    job.operation = AnsibleOperation::CollectPackageInventory;
    job.parameters = AnsibleParameters::CollectPackageInventory;
    let mut result = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
            reboot_required: false,
            duration_ms: 1,
        },
    };

    for (error, expected) in [
        (
            lxcup_agent::PackageInventoryError::ManagerUnavailable,
            "Microsoft.WinGet.Client",
        ),
        (lxcup_agent::PackageInventoryError::Timeout, "timed out"),
        (
            lxcup_agent::PackageInventoryError::TooLarge,
            "response limit",
        ),
        (
            lxcup_agent::PackageInventoryError::InvalidOutput,
            "invalid or unsupported",
        ),
    ] {
        result.response.stderr = error.code().to_owned();
        let (summary, usable) = result_summary(&job, &result);
        assert!(!usable);
        assert!(summary.contains(expected));
    }

    result.response.stderr = "token=must-not-be-shown".to_owned();
    let (summary, usable) = result_summary(&job, &result);
    assert!(!usable);
    assert!(summary.contains("local command failure"));
    assert!(!summary.contains("must-not-be-shown"));
}

#[test]
fn windows_agent_success_summaries_describe_each_allowlisted_operation() {
    let result = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: true,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            reboot_required: false,
            duration_ms: 1,
        },
    };
    for (operation, mode, parameters, expected) in [
        (
            AnsibleOperation::HealthCheck,
            ExecutionMode::Check,
            AnsibleParameters::HealthCheck,
            "health check",
        ),
        (
            AnsibleOperation::CollectPackageInventory,
            ExecutionMode::Check,
            AnsibleParameters::CollectPackageInventory,
            "inventory collected",
        ),
        (
            AnsibleOperation::UpdatePackages,
            ExecutionMode::Apply,
            AnsibleParameters::UpdatePackages {
                packages: vec!["7zip.7zip".to_owned()],
            },
            "installed locally",
        ),
    ] {
        let mut job = package_job(&[], mode);
        job.operation = operation;
        job.parameters = parameters;
        let (summary, usable) = result_summary(&job, &result);
        assert!(usable);
        assert!(summary.contains(expected));
    }
}

#[tokio::test]
async fn authenticated_windows_agent_claims_and_completes_a_local_winget_plan() {
    let (state, target, token) = windows_agent_state().await;
    let JobSubmission::Created(job) = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::UpdatePackages,
            target: ResourceTarget::Target(target.id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Plan,
            parameters: AnsibleParameters::UpdatePackages {
                packages: vec!["7zip.7zip".to_owned()],
            },
            secret_refs: vec![target.agent_secret_ref],
            idempotency_key: "package-plan:test:windows".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .unwrap()
    else {
        panic!("workflow should be created")
    };
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    let JsonBody(claimed) = claim_windows_agent_workflow(
        State(state.clone()),
        headers.clone(),
        JsonBody(AgentWorkflowClaimRequest {
            target_id: target.id.as_uuid(),
        }),
    )
    .await
    .unwrap();
    let command = claimed
        .data
        .expect("the queued package plan should be claimed");
    assert_eq!(command.action, AgentAction::Scan);
    assert_eq!(command.packages, ["7zip.7zip"]);

    let result = AgentWorkflowResult {
            response: lxcup_agent::AgentCommandResponse {
                request_id: Uuid::new_v4(),
                success: true,
                exit_code: 0,
                stdout: r#"[{"id":"7zip.7zip","name":"7-Zip 24.09 (x64)","version":"24.09","candidate_version":"25.01","source":"winget"}]"#.to_owned(),
                stderr: String::new(),
                reboot_required: false,
                duration_ms: 10,
            },
        };
    let response = report_windows_agent_workflow(
        State(state.clone()),
        headers,
        Path(job.id.as_uuid().to_string()),
        JsonBody(result),
    )
    .await
    .unwrap();
    assert_eq!(response, StatusCode::NO_CONTENT);
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::Succeeded
    );
}

#[test]
fn windows_agent_update_claim_contains_only_the_controller_selected_version() {
    let mut job = package_job(&[], ExecutionMode::Plan);
    job.operation = AnsibleOperation::UpdateAgent;
    job.parameters = AnsibleParameters::UpdateAgent {
        agent_version: "0.6.0".to_owned(),
    };
    let plan = workflow_command(job.clone()).unwrap();
    assert_eq!(plan.action, AgentAction::Health);
    assert_eq!(plan.agent_version.as_deref(), Some("0.6.0"));

    job.mode = ExecutionMode::Apply;
    job.status = AnsibleJobStatus::Applying;
    let apply = workflow_command(job).unwrap();
    assert_eq!(apply.action, AgentAction::UpdateAgent);
    assert_eq!(apply.agent_version.as_deref(), Some("0.6.0"));
}

#[tokio::test]
async fn authenticated_windows_agent_claims_and_completes_package_inventory_collection() {
    let (state, target, token) = windows_agent_state().await;
    let JobSubmission::Created(job) = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::CollectPackageInventory,
            target: ResourceTarget::Target(target.id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::CollectPackageInventory,
            secret_refs: vec![target.agent_secret_ref],
            idempotency_key: "package-inventory:windows-agent".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .unwrap()
    else {
        panic!("inventory workflow should be created")
    };
    let headers = agent_headers(&token);
    let JsonBody(claimed) = claim_windows_agent_workflow(
        State(state.clone()),
        headers.clone(),
        JsonBody(AgentWorkflowClaimRequest {
            target_id: target.id.as_uuid(),
        }),
    )
    .await
    .unwrap();
    let command = claimed.data.expect("Windows agent should claim inventory");
    assert_eq!(command.action, AgentAction::Scan);
    assert!(command.packages.is_empty());

    let response = report_windows_agent_workflow(
        State(state.clone()),
        headers,
        Path(job.id.as_uuid().to_string()),
        JsonBody(successful_health_result()),
    )
    .await
    .unwrap();

    assert_eq!(response, StatusCode::NO_CONTENT);
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::Succeeded
    );
}

#[tokio::test]
async fn windows_agent_claim_returns_none_for_missing_or_unsupported_work() {
    let (state, target, token) = windows_agent_state().await;
    let headers = agent_headers(&token);
    let claim = |state: ApiState, headers: HeaderMap, target: &Target| {
        claim_windows_agent_workflow(
            State(state),
            headers,
            JsonBody(AgentWorkflowClaimRequest {
                target_id: target.id.as_uuid(),
            }),
        )
    };
    let no_work = claim(state.clone(), headers.clone(), &target)
        .await
        .unwrap();
    assert!(no_work.0.data.is_none());

    let JobSubmission::Created(job) = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::DeployAgent,
            target: ResourceTarget::Target(target.id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Plan,
            parameters: AnsibleParameters::DeployAgent {
                agent_version: "0.6.0".to_owned(),
            },
            secret_refs: vec![target.agent_secret_ref],
            idempotency_key: "windows-agent-unsupported-operation".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .unwrap()
    else {
        panic!("workflow should be created")
    };
    let unsupported_work = claim(state.clone(), headers, &target).await.unwrap();
    assert!(unsupported_work.0.data.is_none());
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::Queued
    );
}

#[tokio::test]
async fn failed_windows_apply_requires_reconciliation_and_terminal_reports_are_idempotent() {
    let (state, target, token) = windows_agent_state().await;
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
                packages: vec!["7zip.7zip".to_owned()],
            },
            secret_refs: vec![target.agent_secret_ref],
            idempotency_key: "windows-agent-failed-apply".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .unwrap()
    else {
        panic!("workflow should be created")
    };
    let headers = agent_headers(&token);
    let claimed = claim_windows_agent_workflow(
        State(state.clone()),
        headers.clone(),
        JsonBody(AgentWorkflowClaimRequest {
            target_id: target.id.as_uuid(),
        }),
    )
    .await
    .unwrap();
    assert!(claimed.0.data.is_some());

    let failed_result = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: "winget failed".to_owned(),
            reboot_required: false,
            duration_ms: 5,
        },
    };
    assert_eq!(
        report_windows_agent_workflow(
            State(state.clone()),
            headers.clone(),
            Path(job.id.as_uuid().to_string()),
            JsonBody(failed_result.clone()),
        )
        .await
        .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        state.ansible.read().await.job(job.id).unwrap().status,
        AnsibleJobStatus::ReconcileRequired
    );
    assert_eq!(
        report_windows_agent_workflow(
            State(state),
            headers,
            Path(job.id.as_uuid().to_string()),
            JsonBody(failed_result),
        )
        .await
        .unwrap(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn windows_agent_report_rejects_unauthorized_and_oversized_results() {
    let (state, target, token) = windows_agent_state().await;
    let JobSubmission::Created(job) = state
        .ansible
        .write()
        .await
        .submit(AnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target: ResourceTarget::Target(target.id),
            lifecycle: ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            secret_refs: vec![target.agent_secret_ref],
            idempotency_key: "windows-agent-result-validation".to_owned(),
            confirmed: true,
            actor_role: ActorRole::Admin,
        })
        .unwrap()
    else {
        panic!("workflow should be created")
    };
    let mut wrong_headers = HeaderMap::new();
    wrong_headers.insert("authorization", "Bearer wrong-token".parse().unwrap());
    let result = successful_health_result();
    let unauthorized = report_windows_agent_workflow(
        State(state.clone()),
        wrong_headers,
        Path(job.id.as_uuid().to_string()),
        JsonBody(result.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(unauthorized.code, "unauthorized");

    let oversized = AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            stdout: "x".repeat(128 * 1024 + 1),
            ..result.response
        },
    };
    let too_large = report_windows_agent_workflow(
        State(state),
        agent_headers(&token),
        Path(job.id.as_uuid().to_string()),
        JsonBody(oversized),
    )
    .await
    .unwrap_err();
    assert_eq!(too_large.code, "agent_result_too_large");
}

#[test]
fn workflow_status_names_are_stable_for_terminal_and_intermediate_states() {
    assert_eq!(status_name(AnsibleJobStatus::Succeeded), "succeeded");
    assert_eq!(status_name(AnsibleJobStatus::Failed), "failed");
    assert_eq!(
        status_name(AnsibleJobStatus::ReconcileRequired),
        "reconcile_required"
    );
    assert_eq!(status_name(AnsibleJobStatus::Planned), "planned");
    assert_eq!(status_name(AnsibleJobStatus::Checking), "checking");
}

fn agent_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    headers
}

fn successful_health_result() -> AgentWorkflowResult {
    AgentWorkflowResult {
        response: lxcup_agent::AgentCommandResponse {
            request_id: Uuid::new_v4(),
            success: true,
            exit_code: 0,
            stdout: "healthy".to_owned(),
            stderr: String::new(),
            reboot_required: false,
            duration_ms: 5,
        },
    }
}

#[tokio::test]
async fn windows_agent_workflow_claim_rejects_a_wrong_target_token() {
    let (state, target, _) = windows_agent_state().await;
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer invalid".parse().unwrap());
    let error = claim_windows_agent_workflow(
        State(state),
        headers,
        JsonBody(AgentWorkflowClaimRequest {
            target_id: target.id.as_uuid(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "unauthorized");
}

#[tokio::test]
async fn windows_agent_workflow_claim_rejects_legacy_winrm_targets() {
    let (state, target, token) = windows_agent_state().await;
    let mut legacy_winrm_target = Target::new(
        "legacy-windows-target",
        TargetKind::WindowsServer,
        "192.0.2.50",
        TargetTransport::Winrm,
        SecretId::new(),
        target.agent_secret_ref,
    )
    .unwrap();
    legacy_winrm_target.id = target.id;
    let mut store = state.store.write().await;
    store.targets[0] = legacy_winrm_target;
    drop(store);

    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    let error = claim_windows_agent_workflow(
        State(state),
        headers,
        JsonBody(AgentWorkflowClaimRequest {
            target_id: target.id.as_uuid(),
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, "windows_agent_required");
}
