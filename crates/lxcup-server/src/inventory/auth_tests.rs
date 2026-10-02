use super::{audit_action, is_admin_only_route, session_credential, valid_csrf_request};
use axum::http::{HeaderMap, Method};

#[test]
fn administrative_routes_are_explicitly_allowlisted() {
    for path in [
        "/api/v1/users",
        "/api/v1/users/abc",
        "/api/v1/users/abc/password-reset",
        "/api/v1/secrets",
        "/api/v1/secrets/abc",
        "/api/v1/auth/audit",
        "/api/v1/admin/backups",
        "/api/v1/admin/backups/123",
    ] {
        assert!(is_admin_only_route(path), "{path}");
    }
    assert!(!is_admin_only_route("/api/v1/targets"));
    assert!(!is_admin_only_route("/api/v1/auth/password"));
}

#[test]
fn audit_actions_identify_sensitive_changes_without_recording_request_data() {
    assert_eq!(
        audit_action(&Method::POST, "/api/v1/auth/password"),
        "auth.password_changed"
    );
    assert_eq!(
        audit_action(&Method::POST, "/api/v1/targets/target-1/docker/discovery"),
        "docker.discovery_requested"
    );
    assert_eq!(
        audit_action(&Method::DELETE, "/api/v1/update-policies/policy-1"),
        "update_policy.deleted"
    );
    assert_eq!(
        audit_action(&Method::POST, "/api/v1/ansible/jobs/job-1/retry"),
        "workflow.retried"
    );
    assert_eq!(
        audit_action(&Method::POST, "/api/v1/executions/execution-1/abort"),
        "execution.aborted"
    );
}

#[test]
fn cookie_sessions_require_a_matching_csrf_cookie_and_header() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        "lxcup_session=session-secret; lxcup_csrf=csrf-secret"
            .parse()
            .unwrap(),
    );
    assert_eq!(session_credential(&headers), Some(("session-secret", true)));
    assert!(!valid_csrf_request(&headers));

    headers.insert("x-csrf-token", "wrong-token".parse().unwrap());
    assert!(!valid_csrf_request(&headers));
    headers.insert("x-csrf-token", "csrf-secret".parse().unwrap());
    assert!(valid_csrf_request(&headers));

    headers.insert("authorization", "Bearer explicit-token".parse().unwrap());
    assert_eq!(
        session_credential(&headers),
        Some(("explicit-token", false))
    );
}

#[test]
fn duplicate_csrf_cookies_are_rejected() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        "lxcup_csrf=first; lxcup_csrf=second".parse().unwrap(),
    );
    headers.insert("x-csrf-token", "first".parse().unwrap());
    assert!(!valid_csrf_request(&headers));
}
