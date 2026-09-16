use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::{DomainError, DomainResult},
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

impl PlanStatus {
    /// Prüft, ob ein Planstatus direkt erreicht werden darf.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Blocked | Self::Ready | Self::Invalidated)
                | (Self::Blocked, Self::Draft | Self::Invalidated)
                | (Self::Ready, Self::Confirmed | Self::Invalidated)
                | (Self::Confirmed, Self::Invalidated)
        )
    }
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

    /// Ändert den Planstatus nach den Safety-Regeln.
    pub fn transition_to(&mut self, next: PlanStatus) -> DomainResult<()> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition("update plan status"));
        }

        self.status = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PlanStatus, UpdatePlan};
    use crate::{ContainerId, PackageName};

    #[test]
    fn plan_must_be_ready_before_confirmation() {
        let package = PackageName::new("openssl").unwrap();
        let mut plan = UpdatePlan::new(ContainerId::new(101), vec![package], Vec::new()).unwrap();

        assert!(plan.transition_to(PlanStatus::Confirmed).is_err());
        plan.transition_to(PlanStatus::Ready).unwrap();
        plan.transition_to(PlanStatus::Confirmed).unwrap();
        assert_eq!(plan.status, PlanStatus::Confirmed);
    }

    #[test]
    fn blocked_plan_cannot_be_confirmed() {
        let package = PackageName::new("openssl").unwrap();
        let mut plan = UpdatePlan::new(ContainerId::new(101), vec![package], Vec::new()).unwrap();

        plan.transition_to(PlanStatus::Blocked).unwrap();

        assert!(plan.transition_to(PlanStatus::Confirmed).is_err());
    }
}
