use super::*;

#[tokio::test]
async fn docker_inventory_endpoints_manage_in_memory_workloads() {
    let state = ApiState::new();
    state.store.write().await.docker_workloads.insert(
        (ContainerId::new(303), "docker-303".to_owned()),
        DockerWorkloadDto {
            host_container_id: ContainerId::new(303),
            id: "docker-303".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: None,
            labels: vec!["app=web".to_owned()],
            presence: "present".to_owned(),
            change_state: "new".to_owned(),
            management_state: "discovered".to_owned(),
            discovered_at: chrono::Utc::now(),
        },
    );
    let listed = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/303/docker/containers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    let adopted = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/303/docker/containers/docker-303/adopt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(adopted.status(), StatusCode::OK);

    let rejected = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/v1/containers/303/docker/containers/docker-303")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let removed = router(state)
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/v1/containers/303/docker/containers/docker-303")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"confirmed":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);

    for request in [
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/303/docker/discover")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/containers/303/docker/containers/missing/adopt")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/containers/303/docker/containers/missing")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"confirmed":true}"#))
            .unwrap(),
    ] {
        assert_eq!(
            router(ApiState::new().with_auth_config(AuthConfig::disabled()))
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn docker_unavailable_is_audited_without_marking_existing_workloads_missing() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let host_id = ContainerId::new(606);
    let agent_token = "docker-unavailable-token";
    let info = lxcup_agent::AgentInfo {
        agent_id: "windows-agent".to_owned(),
        platform: lxcup_agent::AgentPlatform::Windows,
        hostname: "docker-host".to_owned(),
        version: "0.2.0".to_owned(),
        protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(info, agent_token)),
        )
        .await
        .unwrap();
    });
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new(format!("http://{address}"), agent_token).unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(host_id, RegisteredAgent { client });
    state.store.write().await.docker_workloads.insert(
        (host_id, "previous-container".to_owned()),
        DockerWorkloadDto {
            host_container_id: host_id,
            id: "previous-container".to_owned(),
            name: "previous".to_owned(),
            image: "nginx:stable".to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: vec![],
            started_at: None,
            labels: vec![],
            presence: "present".to_owned(),
            change_state: "unchanged".to_owned(),
            management_state: "managed".to_owned(),
            discovered_at: chrono::Utc::now(),
        },
    );

    let failed = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/606/docker/discover")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
    let latest = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/606/docker/discovery")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(latest.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["status"], "failed");
    assert_eq!(json["data"]["error_code"], "docker_unavailable");

    let inventory = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/606/docker/containers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inventory.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"][0]["presence"], "present");
    agent_task.abort();
}

#[tokio::test]
async fn enrollment_endpoint_is_idempotent_and_queryable() {
    let state = ApiState::new();
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let create_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": 101,
                    "idempotency_key": "request-1"
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let enrollment_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["state"], "requested");

    let second = router(state.clone())
        .oneshot(create_request())
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["data"]["id"], enrollment_id);

    let get = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/enrollments/{enrollment_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
}

#[tokio::test]
async fn enrollment_rejects_key_reuse_and_unknown_containers() {
    let state = ApiState::new();
    let containers = [101, 102]
        .into_iter()
        .map(|id| {
            Container::new(
                ContainerId::new(id),
                NodeId::new(),
                format!("test-lxc-{id}"),
                lxcup_core::OperatingSystem::Debian,
                lxcup_core::ContainerStatus::Running,
            )
            .unwrap()
        })
        .collect();
    state.replace_containers(containers).await;

    let request = |container_id, key| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/enrollments")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "container_id": container_id,
                    "idempotency_key": key
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone())
        .oneshot(request(101, "request-1"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);

    let reused = router(state.clone())
        .oneshot(request(102, "request-1"))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);

    let unknown = router(state)
        .oneshot(request(999, "request-2"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ansible_job_endpoint_accepts_healthcheck_and_is_idempotent() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(101),
        NodeId::new(),
        "test-lxc",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;
    let request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/ansible/jobs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "operation": "health_check",
                    "container_id": 101,
                    "mode": "check",
                    "parameters": {"operation": "health_check"},
                    "idempotency_key": "health-1",
                    "confirmed": false
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let job_id = first_json["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first_json["data"]["status"], "queued");

    let duplicate = router(state.clone()).oneshot(request()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_body = axum::body::to_bytes(duplicate.into_body(), usize::MAX)
        .await
        .unwrap();
    let duplicate_json: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
    assert_eq!(duplicate_json["data"]["id"], job_id);

    let events = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);

    let status = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ansible/jobs/{job_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
}
