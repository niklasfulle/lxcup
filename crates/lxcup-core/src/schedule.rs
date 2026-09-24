//! Deterministic scheduling and threshold primitives used by the controller.
//! The persistence/API layers own storage and dispatch; this module keeps the
//! safety decisions free of clocks, network calls, and side effects.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::TargetId;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleFrequency {
    EveryMinutes(u32),
}

impl ScheduleFrequency {
    pub fn interval(self) -> Duration {
        match self {
            Self::EveryMinutes(minutes) => Duration::minutes(minutes.max(1) as i64),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobSchedule {
    pub id: String,
    pub operation: String,
    pub timezone: String,
    pub target_ids: Vec<TargetId>,
    pub frequency: ScheduleFrequency,
    pub enabled: bool,
    #[serde(default)]
    pub threshold: Option<ThresholdRule>,
    #[serde(default)]
    pub policy_id: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

impl JobSchedule {
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_run_at <= now
    }

    /// Advances from the scheduled slot, not from the poll time. This avoids
    /// drift and makes a missed run produce exactly one catch-up opportunity.
    pub fn advance_after_run(&mut self, now: DateTime<Utc>, error: Option<String>) {
        self.last_run_at = Some(now);
        self.last_error = error;
        let interval = self.frequency.interval();
        let mut next = self.next_run_at + interval;
        while next <= now {
            next += interval;
        }
        self.next_run_at = next;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdMetric {
    CpuBasisPoints,
    MemoryBasisPoints,
    StorageBasisPoints,
    HeartbeatAgeSeconds,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdOperator {
    GreaterThanOrEqual,
    LessThanOrEqual,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThresholdRule {
    pub metric: ThresholdMetric,
    pub operator: ThresholdOperator,
    pub value: u64,
}

impl ThresholdRule {
    pub fn triggered(self, actual: Option<u64>) -> bool {
        let Some(actual) = actual else { return false };
        match self.operator {
            ThresholdOperator::GreaterThanOrEqual => actual >= self.value,
            ThresholdOperator::LessThanOrEqual => actual <= self.value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn schedule() -> JobSchedule {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        JobSchedule {
            id: "inventory".into(),
            operation: "collect_package_inventory".into(),
            timezone: "Europe/Berlin".into(),
            target_ids: vec![TargetId::new()],
            frequency: ScheduleFrequency::EveryMinutes(5),
            enabled: true,
            threshold: None,
            policy_id: None,
            last_run_at: None,
            next_run_at: now,
            last_error: None,
        }
    }

    #[test]
    fn due_schedule_advances_without_drift_after_missed_polls() {
        let mut schedule = schedule();
        let now = schedule.next_run_at + Duration::minutes(17);
        assert!(schedule.is_due(now));
        schedule.advance_after_run(now, None);
        assert_eq!(schedule.next_run_at.minute(), 30);
        assert!(!schedule.is_due(now));
    }

    #[test]
    fn disabled_schedule_never_becomes_due() {
        let mut schedule = schedule();
        schedule.enabled = false;
        assert!(!schedule.is_due(schedule.next_run_at));
    }

    #[test]
    fn thresholds_are_fail_safe_for_missing_samples() {
        let rule = ThresholdRule {
            metric: ThresholdMetric::CpuBasisPoints,
            operator: ThresholdOperator::GreaterThanOrEqual,
            value: 8_000,
        };
        assert!(!rule.triggered(None));
        assert!(!rule.triggered(Some(7_999)));
        assert!(rule.triggered(Some(8_000)));
    }
}
