use super::{
    ApiState, abort_execution, adopt_docker_container, apply_target_docker_image_update,
    auth_login, auth_session, auth_status, change_password, check_target_docker_image_update,
    claim_windows_agent_workflow, confirm_plan, create_ansible_job, create_backup,
    create_enrollment, create_plan, create_schedule, create_secret, create_target,
    create_update_policy, create_user, delete_secret, delete_target, delete_update_policy,
    delete_user, discover_docker_containers, discover_target_docker, download_backup,
    get_agent_health, get_agent_metrics, get_ansible_job, get_ansible_job_events,
    get_docker_discovery, get_enrollment, get_execution, get_execution_result,
    get_package_inventory, get_plan, get_safety, get_secret, get_support_diagnostics, get_target,
    get_target_docker_inventory, get_target_docker_telemetry, get_target_telemetry,
    get_worker_availability, list_ansible_jobs, list_backups, list_container_plans,
    list_containers, list_docker_containers, list_scans, list_schedules, list_secret_audit,
    list_secrets, list_targets, list_telemetry_alerts, list_update_policies, list_user_audit,
    list_users, live_health, logout, metrics, middleware, openapi_document,
    operate_target_docker_container, ready_health, receive_agent_heartbeat, reconcile_ansible_job,
    reconcile_execution, register_agent, remove_docker_container, report_windows_agent_workflow,
    request_middleware, reset_user_password, retry_ansible_job, revoke_agent, revoke_secret,
    rotate_secret, run_execution, run_scan, set_schedule_enabled, start_scan, stream_events,
    terminal_session, update_user,
};
use axum::{
    Router,
    routing::{delete, get, post},
};

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health/live", get(live_health))
        .route("/health/ready", get(ready_health))
        .route("/metrics", get(metrics))
        .route("/api/v1/auth/session", get(auth_session))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/status", get(auth_status))
        .route("/api/v1/auth/login", post(auth_login))
        .route("/api/v1/auth/password", post(change_password))
        .route("/api/v1/auth/audit", get(list_user_audit))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route(
            "/api/v1/users/{user_id}",
            axum::routing::patch(update_user).delete(delete_user),
        )
        .route(
            "/api/v1/users/{user_id}/password-reset",
            post(reset_user_password),
        )
        .route("/api/v1/targets", get(list_targets).post(create_target))
        .route(
            "/api/v1/targets/{target_id}/package-inventory",
            get(get_package_inventory),
        )
        .route(
            "/api/v1/targets/{target_id}/terminal",
            get(terminal_session),
        )
        .route(
            "/api/v1/targets/{target_id}/telemetry",
            get(get_target_telemetry),
        )
        .route(
            "/api/v1/targets/{target_id}/docker/telemetry",
            get(get_target_docker_telemetry),
        )
        .route("/api/v1/telemetry-alerts", get(list_telemetry_alerts))
        .route(
            "/api/v1/targets/{target_id}/docker/discovery",
            get(get_target_docker_inventory).post(discover_target_docker),
        )
        .route(
            "/api/v1/targets/{target_id}/docker/containers/{container_id}/action",
            post(operate_target_docker_container),
        )
        .route(
            "/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check",
            post(check_target_docker_image_update),
        )
        .route(
            "/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-apply",
            post(apply_target_docker_image_update),
        )
        .route(
            "/api/v1/targets/{target_id}",
            get(get_target).delete(delete_target),
        )
        .route(
            "/api/v1/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/schedules/{schedule_id}",
            axum::routing::patch(set_schedule_enabled),
        )
        .route(
            "/api/v1/update-policies",
            get(list_update_policies).post(create_update_policy),
        )
        .route(
            "/api/v1/update-policies/{policy_id}",
            delete(delete_update_policy),
        )
        .route("/api/v1/agents/heartbeat", post(receive_agent_heartbeat))
        .route(
            "/api/v1/agents/workflows/claim",
            post(claim_windows_agent_workflow),
        )
        .route(
            "/api/v1/agents/workflows/{job_id}/result",
            post(report_windows_agent_workflow),
        )
        .route("/api/v1/secrets", get(list_secrets).post(create_secret))
        .route("/api/v1/secrets/audit", get(list_secret_audit))
        .route(
            "/api/v1/secrets/{secret_id}",
            get(get_secret).delete(delete_secret),
        )
        .route("/api/v1/secrets/{secret_id}/rotate", post(rotate_secret))
        .route("/api/v1/secrets/{secret_id}/revoke", post(revoke_secret))
        .route("/api/v1/enrollments", post(create_enrollment))
        .route("/api/v1/enrollments/{enrollment_id}", get(get_enrollment))
        .route(
            "/api/v1/ansible/jobs",
            get(list_ansible_jobs).post(create_ansible_job),
        )
        .route("/api/v1/ansible/jobs/{job_id}", get(get_ansible_job))
        .route(
            "/api/v1/ansible/jobs/{job_id}/retry",
            post(retry_ansible_job),
        )
        .route(
            "/api/v1/ansible/jobs/{job_id}/reconcile",
            post(reconcile_ansible_job),
        )
        .route(
            "/api/v1/ansible/jobs/{job_id}/events",
            get(get_ansible_job_events),
        )
        .route(
            "/api/v1/ansible/worker-availability",
            get(get_worker_availability),
        )
        .route(
            "/api/v1/admin/support-diagnostics",
            get(get_support_diagnostics),
        )
        .route(
            "/api/v1/admin/backups",
            get(list_backups).post(create_backup),
        )
        .route("/api/v1/admin/backups/{backup_id}", get(download_backup))
        .route("/api/v1/containers", get(list_containers))
        .route(
            "/api/v1/containers/{container_id}/scans",
            get(list_scans).post(start_scan),
        )
        .route("/api/v1/scans/{scan_id}/run", post(run_scan))
        .route(
            "/api/v1/containers/{container_id}/plans",
            get(list_container_plans).post(create_plan),
        )
        .route("/api/v1/plans/{plan_id}", get(get_plan))
        .route("/api/v1/plans/{plan_id}/confirm", post(confirm_plan))
        .route(
            "/api/v1/executions/{execution_id}/abort",
            post(abort_execution),
        )
        .route("/api/v1/executions/{execution_id}/run", post(run_execution))
        .route(
            "/api/v1/executions/{execution_id}/reconcile",
            post(reconcile_execution),
        )
        .route("/api/v1/executions/{execution_id}", get(get_execution))
        .route(
            "/api/v1/executions/{execution_id}/result",
            get(get_execution_result),
        )
        .route("/api/v1/executions/{execution_id}/safety", get(get_safety))
        .route(
            "/api/v1/containers/{container_id}/agent",
            post(register_agent),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/health",
            get(get_agent_health),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/metrics",
            get(get_agent_metrics),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/containers",
            get(list_docker_containers),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/discovery",
            get(get_docker_discovery),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/discover",
            post(discover_docker_containers),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/containers/{docker_id}/adopt",
            post(adopt_docker_container),
        )
        .route(
            "/api/v1/containers/{container_id}/docker/containers/{docker_id}",
            axum::routing::delete(remove_docker_container),
        )
        .route(
            "/api/v1/containers/{container_id}/agent/revoke",
            post(revoke_agent),
        )
        .route("/api/v1/openapi.json", get(openapi_document))
        .route("/api/v1/events", get(stream_events))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, request_middleware))
}
