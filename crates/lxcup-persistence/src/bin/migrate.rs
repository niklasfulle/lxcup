use lxcup_persistence::{Database, DatabaseConfig, PersistenceError};
use std::error::Error;
#[cfg(test)]
use std::time::Duration;

#[tokio::main]
async fn main() {
    if should_exit(&run(DatabaseConfig::from_env()).await) {
        std::process::exit(1);
    }
}

async fn run(config: Result<DatabaseConfig, PersistenceError>) -> Result<(), Box<dyn Error>> {
    let config = config?;
    let database = Database::connect(&config).await?;
    database.migrate().await?;
    println!("Database migrations are current.");
    Ok(())
}

fn should_exit(result: &Result<(), Box<dyn Error>>) -> bool {
    if result.is_err() {
        eprintln!(
            "Database migration failed; details omitted to protect deployment configuration."
        );
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_result_controls_exit_without_leaking_error_details() {
        assert!(!should_exit(&Ok(())));
        let error: Box<dyn Error> = Box::new(std::io::Error::other("private database URL"));
        assert!(should_exit(&Err(error)));
    }

    #[tokio::test]
    async fn run_migrates_only_an_explicit_isolated_test_schema() {
        let Ok(database_url) = std::env::var("DATABASE_TEST_URL") else {
            return;
        };
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("explicit PostgreSQL test database must be reachable");
        let schema = format!("migration_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("isolated migration schema must be created");
        let separator = if database_url.contains('?') { '&' } else { '?' };
        let scoped_url = format!("{database_url}{separator}options=-csearch_path%3D{schema}");
        let config = DatabaseConfig::from_values(
            scoped_url,
            3,
            0,
            Duration::from_secs(10),
            Duration::from_secs(10),
            None,
        )
        .expect("test database config must be valid");

        let result = run(Ok(config)).await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .expect("isolated migration schema must be removed");
        admin.close().await;
        assert!(result.is_ok());
    }
}
