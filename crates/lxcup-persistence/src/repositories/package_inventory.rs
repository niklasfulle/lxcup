use super::{Database, PackageInventoryRepository, RepositoryError};
use chrono::{DateTime, Utc};
use lxcup_core::{
    InstalledPackage, PackageInventorySnapshot, PackageName, PackageVersion, TargetId,
};
use sqlx::{Postgres, Row, Transaction};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedPackageInventory {
    pub snapshot: PackageInventorySnapshot,
    pub status: PackageInventoryStatus,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageInventoryStatus {
    Complete,
}

impl PackageInventoryRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }
    /// Replaces a full snapshot atomically; readers cannot observe a partial list.
    pub async fn replace(
        &self,
        snapshot: &PackageInventorySnapshot,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO package_inventory_snapshots (target_id, collected_at, status, package_count) VALUES ($1, $2, 'complete', $3) ON CONFLICT (target_id) DO UPDATE SET collected_at = EXCLUDED.collected_at, status = EXCLUDED.status, package_count = EXCLUDED.package_count").bind(snapshot.target_id.as_uuid()).bind(snapshot.collected_at).bind(snapshot.packages.len() as i32).execute(&mut *transaction).await?;
        sqlx::query("DELETE FROM package_inventory_packages WHERE target_id = $1")
            .bind(snapshot.target_id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        for package in &snapshot.packages {
            insert_package(&mut transaction, snapshot.target_id, package).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
    pub async fn find_latest(
        &self,
        target_id: TargetId,
    ) -> Result<Option<PersistedPackageInventory>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT collected_at, status FROM package_inventory_snapshots WHERE target_id = $1",
        )
        .bind(target_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let collected_at: DateTime<Utc> = row.try_get("collected_at")?;
        let status = match row.try_get::<String, _>("status")?.as_str() {
            "complete" => PackageInventoryStatus::Complete,
            _ => {
                return Err(RepositoryError::InvalidValue {
                    field: "package inventory status",
                });
            }
        };
        let rows = sqlx::query("SELECT package_name, installed_version, candidate_version, architecture, source FROM package_inventory_packages WHERE target_id = $1 ORDER BY package_name, installed_version").bind(target_id.as_uuid()).fetch_all(&self.pool).await?;
        let packages = rows
            .into_iter()
            .map(|row| {
                Ok(InstalledPackage {
                    name: PackageName::new(row.try_get::<String, _>("package_name")?).map_err(
                        |_| RepositoryError::InvalidValue {
                            field: "package name",
                        },
                    )?,
                    version: PackageVersion::new(row.try_get::<String, _>("installed_version")?)
                        .map_err(|_| RepositoryError::InvalidValue {
                            field: "package version",
                        })?,
                    candidate_version: row
                        .try_get::<Option<String>, _>("candidate_version")?
                        .map(PackageVersion::new)
                        .transpose()
                        .map_err(|_| RepositoryError::InvalidValue {
                            field: "package candidate version",
                        })?,
                    architecture: row.try_get("architecture")?,
                    source: row.try_get("source")?,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        Ok(Some(PersistedPackageInventory {
            snapshot: PackageInventorySnapshot {
                target_id,
                collected_at,
                packages,
            },
            status,
        }))
    }
}

async fn insert_package(
    transaction: &mut Transaction<'_, Postgres>,
    target_id: TargetId,
    package: &InstalledPackage,
) -> Result<(), RepositoryError> {
    sqlx::query("INSERT INTO package_inventory_packages (target_id, package_name, installed_version, candidate_version, architecture, source) VALUES ($1, $2, $3, $4, $5, $6)").bind(target_id.as_uuid()).bind(package.name.as_str()).bind(package.version.as_str()).bind(package.candidate_version.as_ref().map(PackageVersion::as_str)).bind(&package.architecture).bind(&package.source).execute(&mut **transaction).await?;
    Ok(())
}
