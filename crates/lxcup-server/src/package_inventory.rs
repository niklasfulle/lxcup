use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use lxcup_core::TargetId;
use serde::Serialize;

use super::{ApiEnvelope, ApiError, ApiState, envelope, parse_uuid};

#[derive(Clone, Debug, Serialize)]
pub struct PackageInventoryDto {
    pub target_id: TargetId,
    pub status: &'static str,
    pub collected_at: Option<DateTime<Utc>>,
    pub packages: Vec<InstalledPackageDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstalledPackageDto {
    pub name: String,
    pub installed_version: String,
    pub candidate_version: Option<String>,
    pub architecture: Option<String>,
    pub source: Option<String>,
}

pub(super) async fn get_package_inventory(
    State(state): State<ApiState>,
    Path(target_id): Path<String>,
) -> Result<Json<ApiEnvelope<PackageInventoryDto>>, ApiError> {
    let target_id = TargetId::from_uuid(parse_uuid(&target_id, "target id")?);
    let target_exists = state
        .store
        .read()
        .await
        .targets
        .iter()
        .any(|target| target.id == target_id);
    if !target_exists {
        return Err(ApiError::not_found("target not found"));
    }
    let latest = match state.repositories.as_ref() {
        Some(repositories) => repositories
            .package_inventory
            .find_latest(target_id)
            .await
            .map_err(|_| ApiError::storage())?,
        None => None,
    };
    let data = match latest {
        Some(inventory) => PackageInventoryDto {
            target_id,
            status: "complete",
            collected_at: Some(inventory.snapshot.collected_at),
            packages: inventory
                .snapshot
                .packages
                .into_iter()
                .map(|package| InstalledPackageDto {
                    name: package.name.as_str().to_owned(),
                    installed_version: package.version.as_str().to_owned(),
                    candidate_version: package
                        .candidate_version
                        .map(|version| version.as_str().to_owned()),
                    architecture: package.architecture,
                    source: package.source,
                })
                .collect(),
        },
        None => PackageInventoryDto {
            target_id,
            status: "not_collected",
            collected_at: None,
            packages: Vec::new(),
        },
    };
    Ok(Json(envelope(data)))
}
