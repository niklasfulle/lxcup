use super::*;
use crate::scheduled_jobs::threshold_value;
use lxcup_agent::AgentInfo;
use lxcup_core::{ThresholdMetric, ThresholdRule};

#[tokio::test]
async fn in_memory_state_restoration_and_safety_results_are_observable() {
    let state = ApiState::new();
    assert_eq!(state.restore_targets().await, 0);
    assert_eq!(state.restore_schedules().await, 0);
    assert_eq!(state.restore_update_policies().await, 0);
    assert_eq!(state.restore_registered_agents().await, 0);

    let execution_id = lxcup_core::ExecutionId::new();
    let missing = get_safety(
        State(state.clone()),
        Path(execution_id.as_uuid().to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "not_found");

    let mut events = state.events.subscribe();
    state
        .record_safety(
            execution_id,
            SafetyDto {
                snapshot: lxcup_safety::SnapshotState::NotRequested,
                healthchecks: Vec::new(),
                reboot: lxcup_safety::RebootRequirement::NotRequired,
                automatic_rollback: false,
            },
        )
        .await;
    let event = events.try_recv().unwrap();
    assert!(matches!(event, ApiEvent::Status { ref resource, .. } if resource == "safety"));

    let Json(response) = get_safety(State(state), Path(execution_id.as_uuid().to_string()))
        .await
        .unwrap();
    assert_eq!(
        response.data.reboot,
        lxcup_safety::RebootRequirement::NotRequired
    );
}

#[tokio::test]
async fn schedule_dispatch_records_a_skipped_target_and_moves_to_the_next_slot() {
    let state = ApiState::new();
    let now = chrono::Utc::now();
    let schedule = JobSchedule {
        id: "missing-target-schedule".to_owned(),
        operation: "health_check".to_owned(),
        timezone: "UTC".to_owned(),
        target_ids: vec![lxcup_core::TargetId::new()],
        frequency: ScheduleFrequency::EveryMinutes(5),
        enabled: true,
        threshold: None,
        policy_id: None,
        last_run_at: None,
        next_run_at: now - chrono::Duration::minutes(1),
        last_error: None,
    };
    state.store.write().await.schedules.push(schedule.clone());

    assert_eq!(state.dispatch_due_schedules().await, 0);
    let store = state.store.read().await;
    let updated = store
        .schedules
        .iter()
        .find(|item| item.id == schedule.id)
        .unwrap();
    assert!(updated.last_run_at.is_some());
    assert!(
        updated
            .last_error
            .as_deref()
            .unwrap()
            .contains("not managed")
    );
    assert!(updated.next_run_at > now);
}

#[test]
fn schedule_thresholds_use_latest_samples_and_fail_closed_without_telemetry() {
    let now = chrono::Utc::now();
    let heartbeat = lxcup_agent::AgentHeartbeat {
        target_id: Uuid::new_v4(),
        info: AgentInfo {
            agent_id: "threshold-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Linux,
            hostname: "threshold-host".to_owned(),
            version: "0.2.0".to_owned(),
            protocol_version: "v1".to_owned(),
        },
        metrics: lxcup_agent::AgentMetrics {
            collected_at: now,
            commands_total: 0,
            commands_failed: 0,
            last_command_at: None,
        },
        sent_at: now,
        telemetry: lxcup_agent::SystemTelemetryWindow {
            samples: vec![lxcup_agent::SystemTelemetrySample {
                collected_at: now,
                cpu_basis_points: Some(2500),
                memory_basis_points: Some(3500),
                storage_basis_points: Some(4500),
                load_1_milli: None,
                network_rx_bytes: None,
                network_tx_bytes: None,
                process_count: None,
            }],
            partial: false,
        },
    };
    for (metric, expected) in [
        (ThresholdMetric::CpuBasisPoints, 2500),
        (ThresholdMetric::MemoryBasisPoints, 3500),
        (ThresholdMetric::StorageBasisPoints, 4500),
    ] {
        assert_eq!(
            threshold_value(
                ThresholdRule {
                    metric,
                    operator: lxcup_core::ThresholdOperator::GreaterThanOrEqual,
                    value: expected,
                },
                &heartbeat,
            ),
            Some(expected)
        );
    }
    assert_eq!(
        threshold_value(
            ThresholdRule {
                metric: ThresholdMetric::HeartbeatAgeSeconds,
                operator: lxcup_core::ThresholdOperator::LessThanOrEqual,
                value: 1,
            },
            &heartbeat,
        ),
        Some(0)
    );
    let mut without_samples = heartbeat;
    without_samples.telemetry.samples.clear();
    assert_eq!(
        threshold_value(
            ThresholdRule {
                metric: ThresholdMetric::CpuBasisPoints,
                operator: lxcup_core::ThresholdOperator::GreaterThanOrEqual,
                value: 1,
            },
            &without_samples,
        ),
        None
    );
}

#[tokio::test]
async fn openapi_route_serves_the_versioned_contract() {
    let response = router(ApiState::new())
        .oneshot(
            Request::builder()
                .uri("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let contract: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(contract["info"]["version"], "v1");
    assert!(contract["paths"]["/api/v1/ansible/jobs"].is_object());
}

#[tokio::test]
async fn event_stream_serializes_published_status_events() {
    use tokio_stream::StreamExt;

    let state = ApiState::new();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    state.publish(ApiEvent::status(
        "test-resource",
        "resource-42".to_owned(),
        "updated",
    ));
    let mut stream = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let event = String::from_utf8(frame.to_vec()).unwrap();
    assert!(event.contains("event: status"));
    assert!(event.contains("resource-42"));
}
