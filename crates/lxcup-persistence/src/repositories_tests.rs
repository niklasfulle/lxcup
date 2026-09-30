use super::*;

#[test]
fn queued_jobs_do_not_block_a_new_worker_run() {
    assert!(!is_active_ansible_job_status("queued"));
    assert!(!is_active_ansible_job_status("succeeded"));
    assert!(is_active_ansible_job_status("applying"));
}

#[test]
fn database_enum_mappings_cover_every_supported_value_and_reject_unknown_values() {
    assert_eq!(
        agent_state_to_db(lxcup_core::AgentConnectionState::Connected),
        "connected"
    );
    assert_eq!(
        agent_state_to_db(lxcup_core::AgentConnectionState::Degraded),
        "degraded"
    );
    assert_eq!(
        agent_state_to_db(lxcup_core::AgentConnectionState::Unreachable),
        "unreachable"
    );

    for status in [
        AnsibleJobStatus::Queued,
        AnsibleJobStatus::Checking,
        AnsibleJobStatus::Planned,
        AnsibleJobStatus::Applying,
        AnsibleJobStatus::ReconcileRequired,
        AnsibleJobStatus::Succeeded,
        AnsibleJobStatus::Failed,
        AnsibleJobStatus::Aborted,
    ] {
        assert!(!ansible_status_to_db(status).is_empty());
    }

    for value in ["unknown", "connected", "disconnected"] {
        assert!(node_status_from_db(value.to_owned()).is_ok());
    }
    assert!(node_status_from_db("other".to_owned()).is_err());

    for value in ["debian", "ubuntu", "unknown:alpine", "fedora"] {
        assert!(matches!(
            os_from_db(value.to_owned()),
            OperatingSystem::Unknown(_) | OperatingSystem::Debian | OperatingSystem::Ubuntu
        ));
    }

    for value in ["running", "stopped", "unknown"] {
        assert!(container_status_from_db(value.to_owned()).is_ok());
    }
    assert!(container_status_from_db("other".to_owned()).is_err());

    for value in ["discovered", "managed", "ignored", "disabled"] {
        assert!(container_management_from_db(value.to_owned()).is_ok());
    }
    assert!(container_management_from_db("other".to_owned()).is_err());

    for value in ["pending", "running", "succeeded", "failed"] {
        assert!(scan_status_from_db(value.to_owned()).is_ok());
    }
    assert!(scan_status_from_db("other".to_owned()).is_err());

    for value in ["security", "normal", "unknown"] {
        assert!(classification_from_db(value.to_owned()).is_ok());
    }
    assert!(classification_from_db("other".to_owned()).is_err());

    for value in ["draft", "blocked", "ready", "confirmed", "invalidated"] {
        assert!(plan_status_from_db(value.to_owned()).is_ok());
    }
    assert!(plan_status_from_db("other".to_owned()).is_err());

    for value in ["install", "upgrade", "remove", "downgrade"] {
        assert!(change_kind_from_db(value.to_owned()).is_ok());
    }
    assert!(change_kind_from_db("other".to_owned()).is_err());

    for value in [
        "queued",
        "running",
        "succeeded",
        "failed",
        "aborted",
        "unknown",
    ] {
        assert!(execution_status_from_db(value.to_owned()).is_ok());
    }
    assert!(execution_status_from_db("other".to_owned()).is_err());
}

#[test]
fn scalar_mappings_preserve_canonical_values() {
    assert_eq!(os_to_db(&OperatingSystem::Debian), "debian");
    assert_eq!(os_to_db(&OperatingSystem::Ubuntu), "ubuntu");
    assert_eq!(
        os_to_db(&OperatingSystem::Unknown("alpine".to_owned())),
        "unknown:alpine"
    );
    assert_eq!(container_id(42).unwrap().value(), 42);
    assert!(container_id(-1).is_err());
    assert_eq!(package_name("nginx".to_owned()).unwrap().as_str(), "nginx");
    assert!(package_name("".to_owned()).is_err());
    assert_eq!(
        package_version("1.2.3".to_owned()).unwrap().as_str(),
        "1.2.3"
    );
    assert!(package_version("".to_owned()).is_err());
}
