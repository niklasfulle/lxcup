use lxcup_persistence::Database;

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "Database migration failed; details omitted to protect deployment configuration."
        );
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::connect_from_env().await?;
    database.migrate().await?;
    println!("Database migrations are current.");
    Ok(())
}
