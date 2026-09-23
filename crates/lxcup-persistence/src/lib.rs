//! PostgreSQL-Verbindung und Persistenzgrenzen für lxcup.

use std::{fmt, time::Duration};

use sqlx::{PgPool, postgres::PgPoolOptions};
use thiserror::Error;

pub mod repositories;
pub mod seeds;

pub use repositories::{
    AgentRegistrationRepository, AnsibleJobRepository, AuditEvent, AuditEventRepository,
    ContainerRepository, DockerWorkloadRepository, EnvironmentRepository, ExecutionEvent,
    ExecutionRepository, ExecutionResultRecord, NodeRepository, Repositories, RepositoryError,
    ScanRepository, TargetRepository, UpdatePlanRepository,
    WorkerHeartbeatRepository,
};

/// Konfiguration für eine PostgreSQL-Verbindung.
#[derive(Clone)]
pub struct DatabaseConfig {
    database_url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    pub connect_timeout: Duration,
    pub idle_timeout: Option<Duration>,
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseConfig")
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("acquire_timeout", &self.acquire_timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .finish()
    }
}

impl DatabaseConfig {
    /// Liest die Konfiguration aus `DATABASE_URL` und optionalen Pool-Variablen.
    pub fn from_env() -> Result<Self, PersistenceError> {
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| PersistenceError::MissingDatabaseUrl)?;
        Self::from_values(
            database_url,
            env_u32("DATABASE_MAX_CONNECTIONS", 5)?,
            env_u32("DATABASE_MIN_CONNECTIONS", 0)?,
            env_duration("DATABASE_ACQUIRE_TIMEOUT_SECS", 30)?,
            env_duration("DATABASE_CONNECT_TIMEOUT_SECS", 10)?,
            env_optional_duration("DATABASE_IDLE_TIMEOUT_SECS")?,
        )
    }

    /// Erstellt Konfiguration explizit, ohne Umgebungsvariablen zu benötigen.
    pub fn from_values(
        database_url: String,
        max_connections: u32,
        min_connections: u32,
        acquire_timeout: Duration,
        connect_timeout: Duration,
        idle_timeout: Option<Duration>,
    ) -> Result<Self, PersistenceError> {
        if database_url.trim().is_empty() {
            return Err(PersistenceError::MissingDatabaseUrl);
        }
        if max_connections == 0 || min_connections > max_connections {
            return Err(PersistenceError::InvalidPoolSettings);
        }

        Ok(Self {
            database_url,
            max_connections,
            min_connections,
            acquire_timeout,
            connect_timeout,
            idle_timeout,
        })
    }
}

/// PostgreSQL-Zugriff mit zentraler Migrationsausführung.
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Baut einen Pool mit den konfigurierten Timeouts auf.
    pub async fn connect(config: &DatabaseConfig) -> Result<Self, PersistenceError> {
        let pool = PgPoolOptions::new()
            .min_connections(config.min_connections)
            .max_connections(config.max_connections)
            .acquire_timeout(config.acquire_timeout.min(config.connect_timeout))
            .idle_timeout(config.idle_timeout)
            .connect(&config.database_url)
            .await
            .map_err(PersistenceError::Connect)?;

        Ok(Self { pool })
    }

    /// Baut den Pool direkt aus der Umgebung auf.
    pub async fn connect_from_env() -> Result<Self, PersistenceError> {
        let config = DatabaseConfig::from_env()?;
        Self::connect(&config).await
    }

    /// Führt alle noch nicht angewendeten versionierten Migrationen aus.
    pub async fn migrate(&self) -> Result<(), PersistenceError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(PersistenceError::Migration)
    }

    /// Gibt den Pool für Repository-Implementierungen frei.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// Fehler der Persistenzgrenze ohne Offenlegung von Zugangsdaten.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("DATABASE_URL is not configured")]
    MissingDatabaseUrl,

    #[error("invalid database pool settings")]
    InvalidPoolSettings,

    #[error("invalid database setting: {name}")]
    InvalidSetting { name: &'static str },

    #[error("database connection failed")]
    Connect(#[source] sqlx::Error),

    #[error("database migration failed")]
    Migration(#[source] sqlx::migrate::MigrateError),
}

fn env_u32(name: &'static str, default: u32) -> Result<u32, PersistenceError> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| PersistenceError::InvalidSetting { name }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(PersistenceError::InvalidSetting { name }),
    }
}

fn env_duration(name: &'static str, default_seconds: u64) -> Result<Duration, PersistenceError> {
    Ok(Duration::from_secs(env_u64(name, default_seconds)?))
}

fn env_optional_duration(name: &'static str) -> Result<Option<Duration>, PersistenceError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(Duration::from_secs(
            value
                .parse()
                .map_err(|_| PersistenceError::InvalidSetting { name })?,
        ))),
        Err(std::env::VarError::NotPresent) => Ok(Some(Duration::from_secs(600))),
        Err(std::env::VarError::NotUnicode(_)) => Err(PersistenceError::InvalidSetting { name }),
    }
}

fn env_u64(name: &'static str, default: u64) -> Result<u64, PersistenceError> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| PersistenceError::InvalidSetting { name }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(PersistenceError::InvalidSetting { name }),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::DatabaseConfig;

    #[test]
    fn debug_output_redacts_database_url() {
        let config = DatabaseConfig::from_values(
            "postgres://user@localhost/lxcup".to_owned(),
            5,
            1,
            Duration::from_secs(30),
            Duration::from_secs(10),
            Some(Duration::from_secs(600)),
        )
        .unwrap();

        let output = format!("{config:?}");

        assert!(!output.contains("postgres://user@localhost/lxcup"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn rejects_invalid_pool_bounds() {
        assert!(
            DatabaseConfig::from_values(
                "postgres://localhost/lxcup".to_owned(),
                0,
                0,
                Duration::from_secs(30),
                Duration::from_secs(10),
                None,
            )
            .is_err()
        );
    }
}
