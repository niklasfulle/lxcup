use super::{safe_compose_identifier, validated_compose_context};

#[test]
fn compose_names_allow_only_bounded_identifiers() {
    assert!(safe_compose_identifier("web-api_1"));
    assert!(!safe_compose_identifier("../web"));
    assert!(!safe_compose_identifier("-f"));
    assert!(!safe_compose_identifier(""));
}

#[test]
fn non_compose_containers_cannot_be_updated_through_compose() {
    assert!(validated_compose_context(&serde_json::json!({"other": "label"})).is_none());
}

#[test]
fn compose_config_must_resolve_inside_the_declared_project_directory() {
    let root = std::env::temp_dir().join(format!("lxcup-compose-{}", uuid::Uuid::new_v4()));
    let outside = std::env::temp_dir().join(format!("lxcup-compose-{}.yaml", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(&outside, "services: {}\n").unwrap();
    let labels = serde_json::json!({
        "com.docker.compose.project": "shop",
        "com.docker.compose.service": "web",
        "com.docker.compose.project.working_dir": root.to_string_lossy().to_string(),
        "com.docker.compose.project.config_files": outside.to_string_lossy().to_string(),
    });
    assert!(validated_compose_context(&labels).is_none());
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir(root).unwrap();
}
