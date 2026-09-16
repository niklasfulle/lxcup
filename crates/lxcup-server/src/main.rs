#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "lxcup server starting");
}
