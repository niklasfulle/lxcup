use super::*;
use crate::RegisteredAgent;
use axum::extract::{Path, State};
use axum::{Json, routing::get};
use std::time::Duration;

async fn docker_discovery_response() -> Json<lxcup_agent::DockerDiscovery> {
    Json(lxcup_agent::DockerDiscovery {
        available: true,
        reason: None,
        collected_at: chrono::Utc::now(),
        containers: vec![lxcup_agent::DockerContainerInfo {
            id: "aabbccddeeff".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            state: "running".to_owned(),
            status: "Up 10 seconds".to_owned(),
            ports: vec!["80/tcp".to_owned()],
            started_at: Some("2026-01-01T00:00:00Z".to_owned()),
            created_at: None,
            image_id: None,
            restart_count: None,
            health: None,
            oom_killed: None,
            labels: vec!["app=web".to_owned()],
        }],
    })
}

#[tokio::test]
async fn repository_backed_docker_inventory_reads_and_mutations_cover_empty_host() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("docker_api_{}", uuid::Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        1,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let state = ApiState::new().with_repositories(lxcup_persistence::Repositories::new(&database));
    let host_id = ContainerId::new(72_001);
    let host = host_id.value().to_string();

    let discovery = get_docker_discovery(State(state.clone()), Path(host.clone()))
        .await
        .unwrap()
        .0;
    assert!(discovery.data.is_none());
    let workloads = list_docker_containers(State(state.clone()), Path(host.clone()))
        .await
        .unwrap()
        .0;
    assert!(workloads.data.is_empty());
    let adopt = adopt_docker_container(
        State(state.clone()),
        Path((host.clone(), "aabbccddeeff".to_owned())),
    )
    .await
    .unwrap_err();
    assert_eq!(adopt.code, "not_found");
    let remove = remove_docker_container(
        State(state),
        Path((host, "aabbccddeeff".to_owned())),
        Json(RemoveDockerWorkloadRequest { confirmed: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(remove.code, "not_found");

    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn repository_backed_discovery_persists_workloads_and_reconciles_missing_containers() {
    let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
        eprintln!("skipped: DATABASE_TEST_URL is not configured");
        return;
    };
    let schema = format!("docker_discovery_api_{}", uuid::Uuid::new_v4().simple());
    let admin_pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let separator = if database_url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    let config = lxcup_persistence::DatabaseConfig::from_values(
        scoped_url,
        1,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    let database = lxcup_persistence::Database::connect(&config).await.unwrap();
    database.migrate().await.unwrap();
    let repositories = lxcup_persistence::Repositories::new(&database);
    let node = lxcup_core::Node::new("docker-api-test", "https://node.invalid").unwrap();
    repositories.nodes.save(&node).await.unwrap();
    let host_id = ContainerId::new(72_002);
    let host = lxcup_core::Container::new(
        host_id,
        node.id,
        "docker-api-host",
        lxcup_core::OperatingSystem::Debian,
        lxcup_core::ContainerStatus::Running,
    )
    .unwrap();
    repositories.containers.save(&host).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let agent_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/docker/containers", get(docker_discovery_response)),
        )
        .await
        .unwrap();
    });
    let client = lxcup_agent::AgentClient::new(
        lxcup_agent::AgentClientConfig::new(format!("http://{address}"), "docker-test-token")
            .unwrap(),
    )
    .unwrap();
    let state = ApiState::new().with_repositories(repositories.clone());
    state
        .agents
        .write()
        .await
        .insert(host_id, RegisteredAgent { client });

    let first = discover_docker_containers(State(state.clone()), Path(host_id.value().to_string()))
        .await
        .unwrap()
        .0;
    assert_eq!(
        first.data.run.status,
        lxcup_core::DockerDiscoveryStatus::Succeeded
    );
    assert_eq!(first.data.run.container_count, 1);
    assert_eq!(first.data.workloads[0].change_state, "new");
    assert_eq!(first.data.workloads[0].presence, "present");

    let adopted = adopt_docker_container(
        State(state.clone()),
        Path((host_id.value().to_string(), "aabbccddeeff".to_owned())),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(adopted.data.management_state, "managed");
    repositories
        .docker_workloads
        .mark_missing_except(host_id, &[])
        .await
        .unwrap();
    let listed = list_docker_containers(State(state), Path(host_id.value().to_string()))
        .await
        .unwrap()
        .0;
    assert_eq!(listed.data[0].presence, "missing");
    assert_eq!(listed.data[0].change_state, "missing");
    assert_eq!(listed.data[0].management_state, "managed");

    agent_task.abort();
    database.pool().close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}
