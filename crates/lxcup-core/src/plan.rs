use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::DomainError,
    ids::{ContainerId, UpdatePlanId},
    update::{PackageName, PackageVersion},
};

/// Lebenszyklus eines Update-Plans.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlanStatus {
    Draft,
    Blocked,
    Ready,
    Confirmed,
    Invalidated,
}

/// Art einer tatsächlich aufgelösten Paketänderung.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PackageChangeKind {
    Install,
    Upgrade,
    Remove,
    Downgrade,
}

/// Eine konkrete Paketänderung aus einem Dry-Run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedPackageChange {
    pub package: PackageName,
    pub from_version: Option<PackageVersion>,
    pub to_version: Option<PackageVersion>,
    pub kind: PackageChangeKind,
}

/// Aufgelöster Plan für eine kontrollierte Update-Ausführung.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePlan {
    pub id: UpdatePlanId,
    pub container_id: ContainerId,
    pub requested_packages: Vec<PackageName>,
    pub resolved_changes: Vec<ResolvedPackageChange>,
    pub status: PlanStatus,
    pub created_at: DateTime<Utc>,
    pub plan_hash: Option<String>,
}

impl UpdatePlan {
    pub fn new(
        container_id: ContainerId,
        requested_packages: Vec<PackageName>,
        resolved_changes: Vec<ResolvedPackageChange>,
    ) -> Result<Self, DomainError> {
        if requested_packages.is_empty() {
            return Err(DomainError::EmptyValue {
                field: "requested packages",
            });
        }

        Ok(Self {
            id: UpdatePlanId::new(),
            container_id,
            requested_packages,
            resolved_changes,
            status: PlanStatus::Draft,
            created_at: Utc::now(),
            plan_hash: None,
        })
    }
}
