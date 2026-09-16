#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    let bind_address =
        std::env::var("LXCUP_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
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
        state = state.with_repositories(lxcup_persistence::Repositories::new(&database));
        tracing::info!("PostgreSQL persistence enabled");
    } else {
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
