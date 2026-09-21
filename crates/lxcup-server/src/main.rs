#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    let bind_address =
        std::env::var("LXCUP_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let dev_seed_enabled = std::env::var("LXCUP_DEV_SEED")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .expect("server bind address must be available");
    let mut state = lxcup_server::ApiState::new();
    if std::env::var_os("DATABASE_URL").is_some() {
        let database = lxcup_persistence::Database::connect_from_env()
            .await
            .expect("configured DATABASE_URL must be reachable");
        database
            .migrate()
            .await
            .expect("database migrations must complete");
        if dev_seed_enabled {
            let seed = lxcup_persistence::seeds::seed_development(database.pool())
                .await
                .expect("development seed must be written");
            state.replace_nodes(vec![seed.node.clone()]).await;
            state.replace_containers(vec![seed.container.clone()]).await;
            tracing::info!(
                container_id = seed.container.id.value(),
                "Development seed loaded"
            );
        }
        state = state.with_repositories(lxcup_persistence::Repositories::new(&database));
        let restored_targets = state.restore_targets().await;
        tracing::info!(restored_targets, "Persisted targets restored");
        let restored = state.restore_registered_agents().await;
        tracing::info!(restored, "Persisted agent registrations restored");
        tracing::info!("PostgreSQL persistence enabled");
    } else {
        if dev_seed_enabled {
            let seed = lxcup_persistence::seeds::development_seed();
            state.replace_nodes(vec![seed.node]).await;
            state.replace_containers(vec![seed.container]).await;
            tracing::info!(container_id = 101, "Development seed loaded in memory");
        }
        tracing::warn!("DATABASE_URL is not configured; using in-memory state");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), %bind_address, "lxcup server starting");
    axum::serve(listener, lxcup_server::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server must run");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
