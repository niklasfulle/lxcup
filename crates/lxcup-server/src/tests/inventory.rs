use super::*;

#[tokio::test]
async fn inventory_endpoints_list_containers_start_scans_and_render_metrics() {
    let state = ApiState::new();
    let node = Node::new("inventory-node", "https://pve.example.test").unwrap();
    let node_id = node.id;
    let container = Container::new(
        ContainerId::new(202),
        node_id,
        "inventory-container",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_nodes(vec![node]).await;
    state.replace_containers(vec![container]).await;

    for request in [Request::builder()
        .uri("/api/v1/containers")
        .body(Body::empty())
        .unwrap()]
    {
        assert_eq!(
            router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/202/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let scan_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/containers/202/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let run_without_agent = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/scans/{scan_id}/run"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_without_agent.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let plan_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/202/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.0",
                            "to_version": "3.1",
                            "kind": "Upgrade",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(plan_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let plan_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/202/plans")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/plans/{plan_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(!scan_id.is_empty());
}

#[tokio::test]
async fn inventory_endpoints_fail_closed_for_invalid_ids_and_unready_nodes() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let node = Node::new("disconnected-node", "https://pve.example.test").unwrap();
    let mut disconnected = node.clone();
    disconnected.status = lxcup_core::NodeStatus::Disconnected;
    state.replace_nodes(vec![disconnected]).await;

    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    for request in [
        Request::builder()
            .uri("/api/v1/containers/not-a-container/scans")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/v1/containers/not-a-container/plans")
            .body(Body::empty())
            .unwrap(),
    ] {
        assert_eq!(
            router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    let missing_scan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/scans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_scan.status(), StatusCode::NOT_FOUND);

    let invalid_plan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": [],
                        "dry_run_changes": []
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_plan.status(), StatusCode::BAD_REQUEST);

    let blocked_plan = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/999/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.1",
                            "to_version": null,
                            "kind": "Remove",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked_plan.status(), StatusCode::CREATED);
    let blocked_body = axum::body::to_bytes(blocked_plan.into_body(), usize::MAX)
        .await
        .unwrap();
    let blocked_id =
        serde_json::from_slice::<serde_json::Value>(&blocked_body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
    let blocked_confirm = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/plans/{blocked_id}/confirm"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked_confirm.status(), StatusCode::BAD_REQUEST);

    for uri in [
        "/api/v1/plans/00000000-0000-0000-0000-000000000001",
        "/api/v1/executions/00000000-0000-0000-0000-000000000001",
        "/api/v1/executions/00000000-0000-0000-0000-000000000001/result",
    ] {
        assert_eq!(
            router(state.clone())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap(),)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    assert_eq!(
        router(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/scans/00000000-0000-0000-0000-000000000001/run")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let abort_missing = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/abort")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(abort_missing.status(), StatusCode::BAD_REQUEST);
    let run_missing = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/run")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_missing.status(), StatusCode::NOT_FOUND);

    let reconcile = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/executions/00000000-0000-0000-0000-000000000001/reconcile")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"observed":"unknown"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reconcile.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn execution_endpoints_confirm_run_reconcile_and_abort() {
    let state = ApiState::new().with_auth_config(AuthConfig::disabled());
    let container = Container::new(
        ContainerId::new(404),
        NodeId::new(),
        "execution-test",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    state.replace_containers(vec![container]).await;

    let plan_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/containers/404/plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "requested_packages": ["openssl"],
                        "dry_run_changes": [{
                            "package": "openssl",
                            "from_version": "3.0",
                            "to_version": "3.1",
                            "kind": "Upgrade",
                            "held": false,
                            "authenticated": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_response.status(), StatusCode::CREATED);
    let plan_body = axum::body::to_bytes(plan_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let plan_id = serde_json::from_slice::<serde_json::Value>(&plan_body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let confirm = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/plans/{plan_id}/confirm"))
                .header("x-actor", "test-suite")
                .header("idempotency-key", "execution-test-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirm.status(), StatusCode::ACCEPTED);
    let confirm_body = axum::body::to_bytes(confirm.into_body(), usize::MAX)
        .await
        .unwrap();
    let execution_id = serde_json::from_slice::<serde_json::Value>(&confirm_body).unwrap()["data"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        let info = lxcup_agent::AgentInfo {
            agent_id: "execution-agent".to_owned(),
            platform: lxcup_agent::AgentPlatform::Windows,
            hostname: "execution-host".to_owned(),
            version: "0.2.0".to_owned(),
            protocol_version: lxcup_agent::PROTOCOL_VERSION.to_owned(),
        };
        axum::serve(
            listener,
            lxcup_agent::agent_router(lxcup_agent::LocalAgentState::new(info, "agent-token")),
        )
        .await
        .unwrap();
    });
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new(format!("http://{address}"), "agent-token").unwrap(),
    )
    .unwrap();
    state
        .agents
        .write()
        .await
        .insert(ContainerId::new(404), RegisteredAgent { client });

    let run = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/run"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run.status(), StatusCode::OK);
    let run_body = axum::body::to_bytes(run.into_body(), usize::MAX)
        .await
        .unwrap();
    let run_json = serde_json::from_slice::<serde_json::Value>(&run_body).unwrap();
    assert_eq!(run_json["data"]["status"], "failed");

    let result = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/executions/{execution_id}/result"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);

    let reconciled = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/reconcile"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"observed":"succeeded"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reconciled.status(), StatusCode::OK);

    let abort = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/executions/{execution_id}/abort"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(abort.status(), StatusCode::BAD_REQUEST);
    agent_task.abort();
}
