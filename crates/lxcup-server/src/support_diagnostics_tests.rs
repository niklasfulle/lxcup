use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use lxcup_agent::{
    AgentHeartbeat, AgentInfo, AgentMetrics, AgentPlatform, DockerTelemetryWindow,
    SystemTelemetryWindow,
};
use lxcup_ansible::{
    AnsibleJobStatus, AnsibleOperation, AnsibleParameters, ExecutionMode, JobFailureCode,
};
use lxcup_core::{
    AnsibleJobId, ContainerId, NodeId, ResourceTarget, SecretId, TargetKind, TargetTransport,
};

#[tokio::test]
async fn support_diagnostics_are_admin_only_and_exclude_secret_values() {
    let state = ApiState::new();
    let denied = match get_support_diagnostics(
        State(state.clone()),
        Extension(ActorRole::Viewer),
        Query(DiagnosticsQuery { days: 7 }),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("viewer role must not read support diagnostics"),
    };
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let Json(envelope) = get_support_diagnostics(
        State(state),
        Extension(ActorRole::Admin),
        Query(DiagnosticsQuery { days: 7 }),
    )
    .await
    .unwrap();
    assert_eq!(envelope.data.schema_version, 1);
    assert!(!envelope.data.privacy.secrets_included);
    assert!(!envelope.data.privacy.raw_worker_logs_included);
    assert!(!envelope.data.privacy.external_upload);
    assert!(envelope.data.resources.is_empty());
    assert!(envelope.data.workflows.is_empty());
}

#[test]
fn support_summary_filter_drops_sensitive_lines_and_bounds_output() {
    let safe = sanitize_summary(
        "Apply completed successfully\npassword=not-for-export\nBearer abc123\nprivate key follows\napi_key=hidden\naccess_key=hidden",
    );
    assert_eq!(safe, "Apply completed successfully");
    assert_eq!(
        sanitize_summary(&"x".repeat(MAX_SUMMARY_CHARS + 10)).len(),
        MAX_SUMMARY_CHARS
    );
}

#[test]
fn support_summary_filter_drops_common_embedded_credentials_and_urls() {
    let mut database_url = url::Url::parse("postgres://db.internal:5432/app").unwrap();
    database_url.set_username("operator").unwrap();
    database_url.set_password(Some("test-value")).unwrap();
    let message = format!(
        concat!(
            "safe failure summary\n",
            "request failed with ghp_0123456789abcdefghijklmnopqrstuvwxyz\n",
            "cloud auth AKIAIOSFODNN7EXAMPLE expired\n",
            "claim eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature_value_123\n",
            "database unavailable at {}\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "c3NoLXJzYS1wcml2YXRlLWtleS1ib2R5LW11c3Qtbm90LWxlYWstZXZlbg\n",
            "-----END OPENSSH PRIVATE KEY-----\n",
            "migration exited with code 1"
        ),
        database_url
    );

    assert_eq!(
        sanitize_summary(&message),
        "safe failure summary migration exited with code 1"
    );
    assert_eq!(diagnostic_text(database_url.as_ref()), "[REDACTED]");
}

#[tokio::test]
async fn support_diagnostics_reject_unbounded_periods() {
    let error = match get_support_diagnostics(
        State(ApiState::new()),
        Extension(ActorRole::Admin),
        Query(DiagnosticsQuery { days: 31 }),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("periods above the export limit must be rejected"),
    };
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn support_diagnostics_reject_zero_period_and_reports_in_memory_worker_state() {
    let state = ApiState::new();
    let error = match get_support_diagnostics(
        State(state.clone()),
        Extension(ActorRole::Admin),
        Query(DiagnosticsQuery { days: 0 }),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("zero-day periods must be rejected"),
    };
    assert_eq!(error.code, "invalid_diagnostics_period");

    let worker = worker_info(&state, Utc::now()).await.unwrap();
    assert!(!worker.available);
    assert_eq!(worker.worker_version, None);
}

#[tokio::test]
async fn support_job_collection_is_recent_sorted_and_bounded() {
    let state = ApiState::new();
    for index in 0..MAX_JOBS + 3 {
        let request = lxcup_ansible::AnsibleJobRequest {
            operation: AnsibleOperation::HealthCheck,
            target: ResourceTarget::Target(lxcup_core::TargetId::new()),
            lifecycle: lxcup_core::ResourceLifecycle::Managed,
            mode: ExecutionMode::Check,
            parameters: AnsibleParameters::HealthCheck,
            secret_refs: Vec::new(),
            idempotency_key: format!("diagnostics-{index}"),
            confirmed: true,
            actor_role: ActorRole::Admin,
        };
        state.ansible.write().await.submit(request).unwrap();
    }

    let recent = latest_jobs(&state, Utc::now() - Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(recent.len(), MAX_JOBS);
    assert!(
        recent
            .windows(2)
            .all(|pair| pair[0].created_at >= pair[1].created_at)
    );
    let first_events = job_events(&state, recent[0].id, Utc::now() - Duration::minutes(1))
        .await
        .unwrap();
    assert!(!first_events.is_empty());

    let future = latest_jobs(&state, Utc::now() + Duration::seconds(1))
        .await
        .unwrap();
    assert!(future.is_empty());
}

#[test]
fn support_bundle_includes_only_curated_log_sources() {
    let events = [
        JobEvent {
            sequence: 1,
            job_id: lxcup_core::AnsibleJobId::new(),
            event: JobEventKind::WorkerLog {
                source: "stdout".to_owned(),
                message: "private output must not be exported".to_owned(),
            },
            created_at: Utc::now(),
        },
        JobEvent {
            sequence: 2,
            job_id: lxcup_core::AnsibleJobId::new(),
            event: JobEventKind::WorkerLog {
                source: "apply".to_owned(),
                message: "Apply completed: 2 tasks changed".to_owned(),
            },
            created_at: Utc::now(),
        },
    ];
    let summaries = events
        .iter()
        .filter_map(|event| match &event.event {
            JobEventKind::WorkerLog { source, message }
                if matches!(source.as_str(), "apply" | "check" | "plan" | "reconcile") =>
            {
                Some(sanitize_summary(message))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(summaries, ["Apply completed: 2 tasks changed"]);
}

#[test]
fn support_diagnostics_summarize_heartbeat_resource_and_all_workflow_events() {
    let now = Utc::now();
    let mut target = lxcup_core::Target::new(
        "diagnostics-target",
        TargetKind::LinuxServer,
        "192.0.2.50",
        TargetTransport::Ssh,
        SecretId::new(),
        SecretId::new(),
    )
    .unwrap();
    target.mark_managed();
    let stale = resource_info(&target, None, now);
    assert!(stale.heartbeat_stale);
    assert_eq!(stale.agent_version, None);

    let heartbeat = AgentHeartbeat {
        target_id: target.id.as_uuid(),
        info: AgentInfo {
            agent_id: "diagnostics-agent".to_owned(),
            platform: AgentPlatform::Linux,
            hostname: "diagnostics-host".to_owned(),
            version: "0.4.0".to_owned(),
            protocol_version: "v1".to_owned(),
        },
        metrics: AgentMetrics {
            collected_at: now,
            commands_total: 0,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: SystemTelemetryWindow::default(),
        docker_telemetry: DockerTelemetryWindow::default(),
    };
    let current = resource_info(&target, Some(&heartbeat), now);
    assert!(!current.heartbeat_stale);
    assert_eq!(current.agent_platform.as_deref(), Some("linux"));
    assert_eq!(current.agent_hostname.as_deref(), Some("diagnostics-host"));
    assert_eq!(current.agent_version.as_deref(), Some("0.4.0"));

    let job = AnsibleJob {
        id: AnsibleJobId::new(),
        operation: AnsibleOperation::UpdatePackages,
        playbook: "packages/update.yml".to_owned(),
        playbook_version: "1".to_owned(),
        target: ResourceTarget::Target(target.id),
        mode: ExecutionMode::Apply,
        parameters: AnsibleParameters::UpdatePackages {
            packages: vec!["curl".to_owned()],
        },
        secret_refs: vec![],
        idempotency_key: "diagnostics-job".to_owned(),
        parameter_hash: "hash".to_owned(),
        status: AnsibleJobStatus::Failed,
        created_at: now,
        updated_at: now,
    };
    let events = [
        event(&job, 1, now - Duration::days(2), JobEventKind::Queued),
        event(
            &job,
            2,
            now,
            JobEventKind::Failed {
                code: JobFailureCode::PlaybookFailed,
            },
        ),
        event(
            &job,
            3,
            now,
            JobEventKind::WorkerLog {
                source: "stdout".to_owned(),
                message: "must not be exported".to_owned(),
            },
        ),
        event(
            &job,
            4,
            now,
            JobEventKind::WorkerLog {
                source: "apply".to_owned(),
                message: "safe summary\npassword=must be removed".to_owned(),
            },
        ),
        event(
            &job,
            5,
            now,
            JobEventKind::WorkerLog {
                source: "check".to_owned(),
                message: "check summary".to_owned(),
            },
        ),
    ];
    let summary = workflow_info(&job, &events, now - Duration::days(1));
    assert_eq!(summary.target, format!("resource:{}", target.id.as_uuid()));
    assert_eq!(summary.status, "failed");
    assert_eq!(summary.failure_codes, ["playbookfailed"]);
    assert_eq!(summary.log_summaries.len(), 2);
    assert_eq!(summary.log_summaries[0].message, "safe summary");
    assert_eq!(summary.log_summaries[1].source, "check");

    let many_summaries = (0..MAX_LOG_SUMMARIES_PER_JOB + 2)
        .map(|index| {
            event(
                &job,
                index as u64 + 10,
                now,
                JobEventKind::WorkerLog {
                    source: "apply".to_owned(),
                    message: format!("summary {index}"),
                },
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        workflow_info(&job, &many_summaries, now - Duration::seconds(1))
            .log_summaries
            .len(),
        MAX_LOG_SUMMARIES_PER_JOB
    );

    assert!(target_label(ResourceTarget::Node(NodeId::new())).starts_with("node:"));
    assert_eq!(
        target_label(ResourceTarget::Container(ContainerId::new(42))),
        "container:42"
    );
}

fn event(
    job: &AnsibleJob,
    sequence: u64,
    created_at: DateTime<Utc>,
    event: JobEventKind,
) -> JobEvent {
    JobEvent {
        sequence,
        job_id: job.id,
        event,
        created_at,
    }
}
