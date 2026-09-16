//! Deterministic, safety-first update planning.

use lxcup_core::{
    ContainerId, DomainError, PackageChangeKind, PackageName, PackageVersion, PlanStatus,
    ResolvedPackageChange, UpdatePlan,
};
use thiserror::Error;

/// A normalized result from an agent/package-manager dry-run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DryRunChange {
    pub package: String,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub kind: PackageChangeKind,
    pub held: bool,
    pub authenticated: bool,
}

/// Input accepted by the planner after discovery and scanning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerInput {
    pub container_id: ContainerId,
    pub requested_packages: Vec<String>,
    pub dry_run_changes: Vec<DryRunChange>,
    pub distribution_upgrade: bool,
}

/// Typed boundary for an agent's package-manager dry-run implementation.
pub trait DryRunAdapter {
    fn dry_run(
        &self,
        container_id: ContainerId,
        packages: &[PackageName],
    ) -> Result<Vec<DryRunChange>, PlannerError>;
}

/// Result of validating a plan against a fresh dry-run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Revalidation {
    Unchanged,
    Changed,
}

/// Builds plans and applies the common safety policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdatePlanner;

impl UpdatePlanner {
    pub fn build(&self, input: PlannerInput) -> Result<UpdatePlan, PlannerError> {
        let requested_packages = input
            .requested_packages
            .into_iter()
            .map(PackageName::new)
            .collect::<Result<Vec<_>, _>>()?;
        let resolved_changes = input
            .dry_run_changes
            .into_iter()
            .map(normalize_change)
            .collect::<Result<Vec<_>, _>>()?;

        let mut plan = UpdatePlan::new(input.container_id, requested_packages, resolved_changes)?;
        let blocked =
            input.distribution_upgrade || plan.resolved_changes.iter().any(is_unsafe_change);
        plan.transition_to(if blocked {
            PlanStatus::Blocked
        } else {
            PlanStatus::Ready
        })?;
        plan.refresh_hash();
        Ok(plan)
    }

    /// Rebuilds the current content and invalidates the old plan if it changed.
    pub fn revalidate(
        &self,
        confirmed_plan: &mut UpdatePlan,
        current: PlannerInput,
    ) -> Result<Revalidation, PlannerError> {
        let fresh = self.build(current)?;
        if confirmed_plan.has_same_content_as(&fresh) {
            return Ok(Revalidation::Unchanged);
        }

        confirmed_plan.transition_to(PlanStatus::Invalidated)?;
        Ok(Revalidation::Changed)
    }
}

fn normalize_change(change: DryRunChange) -> Result<ResolvedPackageChange, PlannerError> {
    if change.held {
        return Err(PlannerError::HeldPackage(change.package));
    }
    if !change.authenticated {
        return Err(PlannerError::UnauthenticatedPackage(change.package));
    }

    Ok(ResolvedPackageChange {
        package: PackageName::new(change.package)?,
        from_version: change.from_version.map(PackageVersion::new).transpose()?,
        to_version: change.to_version.map(PackageVersion::new).transpose()?,
        kind: change.kind,
    })
}

const fn is_unsafe_change(change: &ResolvedPackageChange) -> bool {
    matches!(
        change.kind,
        PackageChangeKind::Remove | PackageChangeKind::Downgrade
    )
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlannerError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("held package cannot be planned: {0}")]
    HeldPackage(String),
    #[error("unauthenticated package cannot be planned: {0}")]
    UnauthenticatedPackage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(changes: Vec<DryRunChange>) -> PlannerInput {
        PlannerInput {
            container_id: ContainerId::new(101),
            requested_packages: vec!["openssl".to_owned()],
            dry_run_changes: changes,
            distribution_upgrade: false,
        }
    }

    fn upgrade() -> DryRunChange {
        DryRunChange {
            package: "openssl".to_owned(),
            from_version: Some("3.0".to_owned()),
            to_version: Some("3.1".to_owned()),
            kind: PackageChangeKind::Upgrade,
            held: false,
            authenticated: true,
        }
    }

    #[test]
    fn safe_upgrade_is_ready_and_hashed() {
        let plan = UpdatePlanner.build(input(vec![upgrade()])).unwrap();

        assert_eq!(plan.status, PlanStatus::Ready);
        assert!(plan.plan_hash.is_some());
    }

    #[test]
    fn removal_and_distribution_upgrade_are_blocked() {
        let mut removal = upgrade();
        removal.kind = PackageChangeKind::Remove;
        assert_eq!(
            UpdatePlanner.build(input(vec![removal])).unwrap().status,
            PlanStatus::Blocked
        );

        let mut distribution = input(vec![upgrade()]);
        distribution.distribution_upgrade = true;
        assert_eq!(
            UpdatePlanner.build(distribution).unwrap().status,
            PlanStatus::Blocked
        );
    }

    #[test]
    fn held_and_unauthenticated_packages_fail_closed() {
        let mut held = upgrade();
        held.held = true;
        assert!(matches!(
            UpdatePlanner.build(input(vec![held])),
            Err(PlannerError::HeldPackage(_))
        ));

        let mut unauthenticated = upgrade();
        unauthenticated.authenticated = false;
        assert!(matches!(
            UpdatePlanner.build(input(vec![unauthenticated])),
            Err(PlannerError::UnauthenticatedPackage(_))
        ));
    }

    #[test]
    fn changed_dry_run_invalidates_confirmed_plan() {
        let planner = UpdatePlanner;
        let mut plan = planner.build(input(vec![upgrade()])).unwrap();
        plan.transition_to(PlanStatus::Confirmed).unwrap();

        let mut changed = upgrade();
        changed.to_version = Some("3.2".to_owned());
        assert_eq!(
            planner.revalidate(&mut plan, input(vec![changed])).unwrap(),
            Revalidation::Changed
        );
        assert_eq!(plan.status, PlanStatus::Invalidated);
    }
}
