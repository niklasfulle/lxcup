use std::sync::Arc;

use lxcup_secrets::{EncryptedFileSecretStore, SecretMasterKey};

fn bind_address(value: Option<String>) -> String {
    value.unwrap_or_else(|| "127.0.0.1:8080".to_owned())
}

fn dev_seed_enabled(value: Option<String>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    let bind_address = bind_address(std::env::var("LXCUP_BIND_ADDRESS").ok());
    let dev_seed_enabled = dev_seed_enabled(std::env::var("LXCUP_DEV_SEED").ok());
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .expect("server bind address must be available");
    let mut state = lxcup_server::ApiState::new();
    if let (Ok(secret_root), Ok(master_key)) = (
        std::env::var("LXCUP_SECRET_STORE_DIR"),
        SecretMasterKey::from_env("LXCUP_SECRET_MASTER_KEY"),
    ) {
        let secret_store = EncryptedFileSecretStore::new(secret_root, master_key, [])
            .expect("configured encrypted secret store must be available");
        state = state.with_secret_store(Arc::new(secret_store));
        tracing::info!("Encrypted shared secret store enabled");
    } else {
        tracing::warn!(
            "Encrypted secret store is not configured; worker execution remains disabled"
        );
    }
    if std::env::var_os("DATABASE_URL").is_some() {
        let database = lxcup_persistence::Database::connect_from_env()
            .await
            .expect("configured DATABASE_URL must be reachable");
        database
            .migrate()
            .await
            .expect("database migrations must complete");
        if dev_seed_enabled {
            tracing::info!("Development seed is disabled; resources are registered manually");
        }
        state = state.with_repositories(lxcup_persistence::Repositories::new(&database));
        let restored_targets = state.restore_targets().await;
        tracing::info!(restored_targets, "Persisted targets restored");
        let restored = state.restore_registered_agents().await;
        tracing::info!(restored, "Persisted agent registrations restored");
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

#[cfg(test)]
mod tests {
    use super::{bind_address, dev_seed_enabled};

    #[test]
    fn server_defaults_are_safe_and_seed_flag_is_case_insensitive() {
        assert_eq!(bind_address(None), "127.0.0.1:8080");
        assert_eq!(
            bind_address(Some("0.0.0.0:9000".to_owned())),
            "0.0.0.0:9000"
        );
        assert!(dev_seed_enabled(Some("TRUE".to_owned())));
        assert!(!dev_seed_enabled(Some("false".to_owned())));
        assert!(!dev_seed_enabled(None));
    }
}
