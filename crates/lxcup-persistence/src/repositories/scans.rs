use super::{
    Database, RepositoryError, Scan, ScanId, ScanRepository, available_update_from_row,
    classification_to_db, save_scan, scan_from_row,
};

impl ScanRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, scan: &Scan) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        save_scan(&mut transaction, scan).await?;
        for update in &scan.updates {
            sqlx::query(
                "INSERT INTO scan_updates (scan_id, package_name, installed_version, candidate_version, classification, held) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(scan.id.as_uuid())
            .bind(update.package.as_str())
            .bind(update.installed_version.as_str())
            .bind(update.candidate_version.as_str())
            .bind(classification_to_db(update.classification))
            .bind(update.held)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: ScanId) -> Result<Option<Scan>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT id, container_id, status, created_at, completed_at FROM scans WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let update_rows = sqlx::query(
            "SELECT package_name, installed_version, candidate_version, classification, held FROM scan_updates WHERE scan_id = $1 ORDER BY package_name",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        let mut scan = scan_from_row(row)?;
        scan.updates = update_rows
            .into_iter()
            .map(available_update_from_row)
            .collect::<Result<_, _>>()?;
        Ok(Some(scan))
    }
}
