use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    error::{DomainError, DomainResult},
    ids::{ContainerId, ScanId},
    update::AvailableUpdate,
};

/// Lebenszyklus eines Update-Scans.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ScanStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

impl ScanStatus {
    /// Prüft, ob ein Scanstatus direkt erreicht werden darf.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running) | (Self::Running, Self::Succeeded | Self::Failed)
        )
    }
}

/// Ergebnis eines Update-Scans für einen Container.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Scan {
    pub id: ScanId,
    pub container_id: ContainerId,
    pub status: ScanStatus,
    pub updates: Vec<AvailableUpdate>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Scan {
    pub fn new(container_id: ContainerId) -> Self {
        Self {
            id: ScanId::new(),
            container_id,
            status: ScanStatus::Pending,
            updates: Vec::new(),
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    /// Ändert den Scanstatus und setzt Abschlusszeitpunkte konsistent.
    pub fn transition_to(&mut self, next: ScanStatus, at: DateTime<Utc>) -> DomainResult<()> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition("scan status"));
        }

        self.status = next;
        if matches!(next, ScanStatus::Succeeded | ScanStatus::Failed) {
            self.completed_at = Some(at);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{Scan, ScanStatus};
    use crate::ContainerId;

    #[test]
    fn scan_completes_with_timestamp() {
        let mut scan = Scan::new(ContainerId::new(101));
        let now = Utc::now();

        scan.transition_to(ScanStatus::Running, now).unwrap();
        scan.transition_to(ScanStatus::Succeeded, now).unwrap();

        assert_eq!(scan.status, ScanStatus::Succeeded);
        assert_eq!(scan.completed_at, Some(now));
    }

    #[test]
    fn scan_cannot_skip_running_state() {
        let mut scan = Scan::new(ContainerId::new(101));

        assert!(
            scan.transition_to(ScanStatus::Succeeded, Utc::now())
                .is_err()
        );
    }
}
