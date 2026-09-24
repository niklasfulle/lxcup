//! Explicit update-policy rules. Policies are evaluated before a plan can be
//! confirmed; they never grant permission to bypass actor/RBAC checks.

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::TargetId;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateRisk {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePolicy {
    pub id: String,
    pub allowed_targets: Vec<TargetId>,
    pub allowed_packages: Vec<String>,
    pub maintenance_start_minute: u16,
    pub maintenance_end_minute: u16,
    pub timezone: String,
    pub maximum_risk: UpdateRisk,
    pub enabled: bool,
}

impl UpdatePolicy {
    pub fn allows(
        &self,
        target: TargetId,
        package: &str,
        risk: UpdateRisk,
        at: DateTime<Utc>,
    ) -> bool {
        if !self.enabled
            || !self.allowed_targets.contains(&target)
            || risk > self.maximum_risk
            || (!self.allowed_packages.is_empty()
                && !self.allowed_packages.iter().any(|item| item == package))
        {
            return false;
        }
        let minute = (at.hour() * 60 + at.minute()) as u16;
        if self.maintenance_start_minute <= self.maintenance_end_minute {
            (self.maintenance_start_minute..=self.maintenance_end_minute).contains(&minute)
        } else {
            minute >= self.maintenance_start_minute || minute <= self.maintenance_end_minute
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_blocks_target_package_risk_and_window_violations() {
        let target = TargetId::new();
        let policy = UpdatePolicy {
            id: "safe".into(),
            allowed_targets: vec![target],
            allowed_packages: vec!["curl".into()],
            maintenance_start_minute: 60,
            maintenance_end_minute: 120,
            timezone: "UTC".into(),
            maximum_risk: UpdateRisk::Medium,
            enabled: true,
        };
        let in_window = DateTime::parse_from_rfc3339("2026-01-01T01:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(policy.allows(target, "curl", UpdateRisk::Low, in_window));
        assert!(!policy.allows(target, "openssl", UpdateRisk::Low, in_window));
        assert!(!policy.allows(target, "curl", UpdateRisk::High, in_window));
        assert!(!policy.allows(
            target,
            "curl",
            UpdateRisk::Low,
            in_window - chrono::Duration::hours(1)
        ));
    }

    #[test]
    fn overnight_maintenance_windows_wrap_midnight() {
        let target = TargetId::new();
        let policy = UpdatePolicy {
            id: "overnight".into(),
            allowed_targets: vec![target],
            allowed_packages: Vec::new(),
            maintenance_start_minute: 23 * 60,
            maintenance_end_minute: 60,
            timezone: "UTC".into(),
            maximum_risk: UpdateRisk::High,
            enabled: true,
        };
        let late = DateTime::parse_from_rfc3339("2026-01-01T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let early = DateTime::parse_from_rfc3339("2026-01-01T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(policy.allows(target, "curl", UpdateRisk::High, late));
        assert!(policy.allows(target, "curl", UpdateRisk::High, early));
    }
}
