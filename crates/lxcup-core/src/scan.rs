use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
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
}
