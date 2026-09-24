use std::sync::Arc;
use std::time::Duration;

use lxcup_secrets::{EncryptedFileSecretStore, SecretMasterKey};

fn bind_address(value: Option<String>) -> String {
    value.unwrap_or_else(|| "127.0.0.1:8080".to_owned())
}

fn dev_seed_enabled(value: Option<String>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

struct StartupConfig {
    bind_address: String,
    dev_seed_enabled: bool,
    production: bool,
    auth: lxcup_server::AuthConfig,
    secret_store: Option<(String, SecretMasterKey)>,
    database_configured: bool,
}

fn startup_config_from_values(
    bind: Option<String>,
    dev_seed: Option<String>,
    environment: Option<String>,
    auth: lxcup_server::AuthConfig,
    secret_root: Option<String>,
    master_key: Option<SecretMasterKey>,
    database_configured: bool,
) -> StartupConfig {
    let secret_store = secret_root.zip(master_key);
    StartupConfig {
        bind_address: bind_address(bind),
        dev_seed_enabled: dev_seed_enabled(dev_seed),
        production: environment.is_some_and(|value| value.eq_ignore_ascii_case("production")),
        auth,
        secret_store,
        database_configured,
    }
}

#[tokio::main]
async fn main() {
    lxcup_observability::init("lxcup-server");
    let secret_root = std::env::var("LXCUP_SECRET_STORE_DIR").ok();
    let master_key = SecretMasterKey::from_env("LXCUP_SECRET_MASTER_KEY");
    let config = startup_config_from_values(
        std::env::var("LXCUP_BIND_ADDRESS").ok(),
        std::env::var("LXCUP_DEV_SEED").ok(),
        std::env::var("LXCUP_ENV").ok(),
        lxcup_server::AuthConfig::from_env(),
        secret_root,
        master_key.ok(),
        std::env::var_os("DATABASE_URL").is_some(),
    );
    run(config, shutdown_signal())
        .await
        .expect("server must run");
}

async fn run(
    config: StartupConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let StartupConfig {
        bind_address,
        dev_seed_enabled,
        production,
        auth,
        secret_store,
        database_configured,
    } = config;
    if production && !auth.is_production_ready() {
        panic!(
            "production requires LXCUP_AUTH_REQUIRED=true and distinct role tokens of at least 16 characters"
        );
    }
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .expect("server bind address must be available");
    let mut state = lxcup_server::ApiState::new().with_auth_config(auth);
    if let Some((secret_root, master_key)) = secret_store {
        let secret_store = EncryptedFileSecretStore::new(secret_root, master_key, [])
            .expect("configured encrypted secret store must be available");
        state = state.with_secret_store(Arc::new(secret_store));
        tracing::info!("Encrypted shared secret store enabled");
    } else {
        if production {
            panic!("production requires an encrypted secret store and LXCUP_SECRET_MASTER_KEY");
        }
        tracing::warn!(
            "Encrypted secret store is not configured; worker execution remains disabled"
        );
    }
    if database_configured {
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
        let restored_schedules = state.restore_schedules().await;
        tracing::info!(restored_schedules, "Persisted schedules restored");
        let restored = state.restore_registered_agents().await;
        tracing::info!(restored, "Persisted agent registrations restored");
        tracing::info!("PostgreSQL persistence enabled");
    } else {
        if production {
            panic!("production requires DATABASE_URL");
        }
        tracing::warn!("DATABASE_URL is not configured; using in-memory state");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), %bind_address, "lxcup server starting");
    let scheduler_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let dispatched = scheduler_state.dispatch_due_schedules().await;
            if dispatched > 0 {
                tracing::info!(dispatched, "scheduled jobs queued");
            }
        }
    });
    axum::serve(listener, lxcup_server::router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::{bind_address, dev_seed_enabled, run, startup_config_from_values};
    use lxcup_secrets::SecretMasterKey;
    use std::time::Duration;

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

    #[tokio::test]
    async fn server_runs_in_memory_and_accepts_graceful_shutdown() {
        for (seed, secret_store) in [(None, None), (Some("TRUE"), Some("test-secret-store"))] {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let bind = probe.local_addr().unwrap().to_string();
            drop(probe);
            let secret_root = secret_store.map(|_| {
                let root = std::env::temp_dir()
                    .join(format!("lxcup-server-secrets-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                root
            });
            let config = startup_config_from_values(
                Some(bind),
                seed.map(str::to_owned),
                None,
                lxcup_server::AuthConfig::from_env(),
                secret_root.as_ref().map(|root| root.display().to_string()),
                secret_root
                    .as_ref()
                    .map(|_| SecretMasterKey::from_bytes([9; 32])),
                false,
            );
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(run(config, async move {
                let _ = shutdown_rx.await;
            }));
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown_tx.send(()).unwrap();
            server.await.unwrap().unwrap();
            if let Some(root) = secret_root {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn configured_database_without_url_fails_during_startup() {
        if std::env::var_os("DATABASE_URL").is_some() {
            return;
        }
        let config = startup_config_from_values(
            Some("127.0.0.1:0".to_owned()),
            None,
            None,
            lxcup_server::AuthConfig::from_env(),
            None,
            None,
            true,
        );
        let result = tokio::spawn(run(config, std::future::pending())).await;
        assert!(result.unwrap_err().is_panic());
    }

    #[test]
    fn startup_config_normalizes_environment_values_and_secret_store_pairing() {
        let auth = lxcup_server::AuthConfig::from_env();
        let config = startup_config_from_values(
            None,
            Some("true".to_owned()),
            Some("PRODUCTION".to_owned()),
            auth,
            Some("secrets".to_owned()),
            Some(SecretMasterKey::from_bytes([5; 32])),
            false,
        );
        assert_eq!(config.bind_address, "127.0.0.1:8080");
        assert!(config.dev_seed_enabled);
        assert!(config.production);
        assert!(config.secret_store.is_some());
        let config = startup_config_from_values(
            Some("0.0.0.0:9000".to_owned()),
            None,
            Some("dev".to_owned()),
            lxcup_server::AuthConfig::from_env(),
            Some("orphaned-root".to_owned()),
            None,
            true,
        );
        assert_eq!(config.bind_address, "0.0.0.0:9000");
        assert!(!config.production);
        assert!(config.secret_store.is_none());
        assert!(config.database_configured);
    }
}
