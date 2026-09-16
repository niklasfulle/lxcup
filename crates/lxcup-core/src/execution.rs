use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{ExecutionId, UpdatePlanId};

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
}
