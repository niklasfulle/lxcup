use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

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

#[derive(Serialize)]
struct CanonicalPlan<'a> {
    container_id: u64,
    requested_packages: Vec<&'a str>,
    resolved_changes: Vec<CanonicalPackageChange<'a>>,
}

#[derive(Serialize)]
struct CanonicalPackageChange<'a> {
    package: &'a str,
    from_version: Option<&'a str>,
    to_version: Option<&'a str>,
    kind: PackageChangeKind,
}

impl CanonicalPackageChange<'_> {
    fn sort_key(&self) -> (&str, Option<&str>, Option<&str>, u8) {
        (
            self.package,
            self.from_version,
            self.to_version,
            match self.kind {
                PackageChangeKind::Install => 0,
                PackageChangeKind::Upgrade => 1,
                PackageChangeKind::Remove => 2,
                PackageChangeKind::Downgrade => 3,
            },
        )
    }
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

    /// Erzeugt den stabilen Hash der fachlich relevanten Planinhalte.
    pub fn calculate_hash(&self) -> String {
        let canonical = self.canonical_representation();
        let bytes = serde_json::to_vec(&canonical).expect("canonical plan is serializable");
        let digest = Sha256::digest(bytes);

        let mut hash = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut hash, "{byte:02x}").expect("writing to a String cannot fail");
        }
        hash
    }

    /// Speichert den aktuellen Hash für die spätere Revalidierung.
    pub fn refresh_hash(&mut self) {
        self.plan_hash = Some(self.calculate_hash());
    }

    /// Vergleicht zwei Pläne ohne IDs, Status oder Zeitstempel einzubeziehen.
    pub fn has_same_content_as(&self, other: &Self) -> bool {
        self.calculate_hash() == other.calculate_hash()
    }

    fn canonical_representation(&self) -> CanonicalPlan<'_> {
        let mut requested_packages: Vec<_> = self
            .requested_packages
            .iter()
            .map(|package| package.as_str())
            .collect();
        requested_packages.sort_unstable();

        let mut resolved_changes: Vec<_> = self
            .resolved_changes
            .iter()
            .map(|change| CanonicalPackageChange {
                package: change.package.as_str(),
                from_version: change.from_version.as_ref().map(PackageVersion::as_str),
                to_version: change.to_version.as_ref().map(PackageVersion::as_str),
                kind: change.kind,
            })
            .collect();
        resolved_changes.sort_unstable_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        CanonicalPlan {
            container_id: self.container_id.value(),
            requested_packages,
            resolved_changes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PackageChangeKind, PlanStatus, ResolvedPackageChange, UpdatePlan};
    use crate::{ContainerId, PackageName, PackageVersion};

    fn plan(requested: &[&str], changes: &[(&str, &str, &str)]) -> UpdatePlan {
        let requested_packages = requested
            .iter()
            .map(|package| PackageName::new(*package).unwrap())
            .collect();
        let resolved_changes = changes
            .iter()
            .map(|(package, from, to)| ResolvedPackageChange {
                package: PackageName::new(*package).unwrap(),
                from_version: Some(PackageVersion::new(*from).unwrap()),
                to_version: Some(PackageVersion::new(*to).unwrap()),
                kind: PackageChangeKind::Upgrade,
            })
            .collect();

        UpdatePlan::new(ContainerId::new(101), requested_packages, resolved_changes).unwrap()
    }

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

    #[test]
    fn equivalent_plans_have_the_same_hash() {
        let first = plan(
            &["openssl", "libc6"],
            &[
                ("openssl", "3.5.0-1", "3.5.1-1"),
                ("libc6", "2.41-8", "2.41-9"),
            ],
        );
        let second = plan(
            &["libc6", "openssl"],
            &[
                ("libc6", "2.41-8", "2.41-9"),
                ("openssl", "3.5.0-1", "3.5.1-1"),
            ],
        );

        assert!(first.has_same_content_as(&second));
        assert_eq!(first.calculate_hash(), second.calculate_hash());
    }

    #[test]
    fn package_change_modifies_the_hash() {
        let original = plan(&["openssl"], &[("openssl", "3.5.0-1", "3.5.1-1")]);
        let changed = plan(&["openssl"], &[("openssl", "3.5.0-1", "3.5.2-1")]);

        assert_ne!(original.calculate_hash(), changed.calculate_hash());
    }

    #[test]
    fn refresh_hash_stores_current_plan_hash() {
        let mut plan = plan(&["openssl"], &[("openssl", "3.5.0-1", "3.5.1-1")]);
        let expected = plan.calculate_hash();

        plan.refresh_hash();

        assert_eq!(plan.plan_hash.as_deref(), Some(expected.as_str()));
    }
}
