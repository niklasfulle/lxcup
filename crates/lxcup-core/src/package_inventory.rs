use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{PackageName, PackageVersion, TargetId};

/// A complete, point-in-time package inventory received from a managed target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackageInventorySnapshot {
    pub target_id: TargetId,
    pub collected_at: DateTime<Utc>,
    pub packages: Vec<InstalledPackage>,
}

/// A package installed on a target. Architecture and source are optional because
/// package managers do not consistently expose them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstalledPackage {
    pub name: PackageName,
    pub version: PackageVersion,
    pub candidate_version: Option<PackageVersion>,
    pub architecture: Option<String>,
    pub source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_optional_package_metadata_without_secret_material() {
        let snapshot = PackageInventorySnapshot {
            target_id: TargetId::new(),
            collected_at: Utc::now(),
            packages: vec![InstalledPackage {
                name: PackageName::new("curl").unwrap(),
                version: PackageVersion::new("8.5.0-2").unwrap(),
                candidate_version: Some(PackageVersion::new("8.6.0-1").unwrap()),
                architecture: Some("amd64".to_owned()),
                source: None,
            }],
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("curl"));
        assert!(json.contains("amd64"));
        assert!(!json.contains("secret"));
    }
}
