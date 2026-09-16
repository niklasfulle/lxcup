use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::{DomainError, DomainResult},
    ids::{ExecutionId, UpdatePlanId},
};

/// Lebenszyklus einer Plan-Ausführung.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ExecutionStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Aborted,
    Unknown,
}

impl ExecutionStatus {
    /// Prüft, ob ein Ausführungsstatus direkt erreicht werden darf.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Running | Self::Aborted)
                | (
                    Self::Running,
                    Self::Succeeded | Self::Failed | Self::Aborted | Self::Unknown
                )
                | (
                    Self::Unknown,
                    Self::Succeeded | Self::Failed | Self::Aborted
                )
        )
    }
}

/// Persistierter Lauf einer bestätigten Update-Ausführung.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Execution {
    pub id: ExecutionId,
    pub plan_id: UpdatePlanId,
    pub status: ExecutionStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Execution {
    pub fn new(plan_id: UpdatePlanId) -> Self {
        Self {
            id: ExecutionId::new(),
            plan_id,
            status: ExecutionStatus::Queued,
            started_at: None,
            finished_at: None,
        }
    }

    /// Ändert den Ausführungsstatus und pflegt die Zeitpunkte.
    pub fn transition_to(&mut self, next: ExecutionStatus, at: DateTime<Utc>) -> DomainResult<()> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition("execution status"));
        }

        self.status = next;
        if next == ExecutionStatus::Running && self.started_at.is_none() {
            self.started_at = Some(at);
        }
        if matches!(
            next,
            ExecutionStatus::Succeeded | ExecutionStatus::Failed | ExecutionStatus::Aborted
        ) {
            self.finished_at = Some(at);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{Execution, ExecutionStatus};
    use crate::UpdatePlanId;

    #[test]
    fn execution_records_start_and_finish() {
        let mut execution = Execution::new(UpdatePlanId::new());
        let now = Utc::now();

        execution
            .transition_to(ExecutionStatus::Running, now)
            .unwrap();
        execution
            .transition_to(ExecutionStatus::Succeeded, now)
            .unwrap();

        assert_eq!(execution.started_at, Some(now));
        assert_eq!(execution.finished_at, Some(now));
    }

    #[test]
    fn terminal_execution_cannot_run_again() {
        let mut execution = Execution::new(UpdatePlanId::new());
        let now = Utc::now();

        execution
            .transition_to(ExecutionStatus::Running, now)
            .unwrap();
        execution
            .transition_to(ExecutionStatus::Succeeded, now)
            .unwrap();

        assert!(
            execution
                .transition_to(ExecutionStatus::Running, now)
                .is_err()
        );
    }
}
