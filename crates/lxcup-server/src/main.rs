#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    let bind_address =
        std::env::var("LXCUP_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .expect("server bind address must be available");
    tracing::info!(version = env!("CARGO_PKG_VERSION"), %bind_address, "lxcup server starting");
    axum::serve(
        listener,
        lxcup_server::router(lxcup_server::ApiState::new()),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server must run");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
