use super::ApiState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

pub(crate) async fn live_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub(crate) async fn ready_health(State(state): State<ApiState>) -> impl IntoResponse {
    let targets_ready = state
        .store
        .read()
        .await
        .targets
        .iter()
        .all(|target| target.state != lxcup_core::TargetState::Disabled);
    let database_ready = match state.repositories.as_ref() {
        Some(repositories) => repositories.ansible_jobs.ping().await.is_ok(),
        None => true,
    };
    let ready = targets_ready && database_ready;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": if ready { "ready" } else { "degraded" },
            "checks": {
                "database": if database_ready { "ready" } else { "not_ready" },
                "targets": if targets_ready { "ready" } else { "degraded" },
            }
        })),
    )
}

pub(crate) async fn metrics(State(state): State<ApiState>) -> impl IntoResponse {
    let mut rendered = state.metrics.render();
    let (queue, last_seen) = if let Some(repositories) = state.repositories.as_ref() {
        let (queue, heartbeat) = tokio::join!(
            repositories.ansible_jobs.queue_metrics(),
            repositories.worker_heartbeats.latest()
        );
        (queue.unwrap_or_default(), heartbeat.ok().flatten())
    } else {
        (Default::default(), None)
    };
    let queue_age = queue
        .oldest_queued_at
        .map(|created| (chrono::Utc::now() - created).num_seconds().max(0))
        .unwrap_or(0);
    let heartbeat_age = last_seen
        .map(|seen| (chrono::Utc::now() - seen).num_seconds().max(0))
        .unwrap_or(-1);
    let worker_available = i32::from((0..=10).contains(&heartbeat_age));
    rendered.push_str(&format!(
        "# TYPE lxcup_ansible_jobs_queued gauge\nlxcup_ansible_jobs_queued {}\n# TYPE lxcup_ansible_jobs_failed gauge\nlxcup_ansible_jobs_failed {}\n# TYPE lxcup_ansible_queue_age_seconds gauge\nlxcup_ansible_queue_age_seconds {}\n# TYPE lxcup_worker_heartbeat_age_seconds gauge\nlxcup_worker_heartbeat_age_seconds {}\n# TYPE lxcup_worker_available gauge\nlxcup_worker_available {}\n",
        queue.queued_jobs, queue.failed_jobs, queue_age, heartbeat_age, worker_available
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        rendered,
    )
}
