use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ContainerId, DomainError, EnrollmentId, error::DomainResult};

/// Lifecycle of a container enrollment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentState {
    Requested,
    Discovering,
    InstallingAgent,
    RegisteringAgent,
    Connected,
    Failed,
    Disabled,
}

impl EnrollmentState {
    /// Returns whether the lifecycle may move from this state to the next one.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Requested,
                Self::Requested | Self::Discovering | Self::Failed
            ) | (
                Self::Discovering,
                Self::Discovering | Self::InstallingAgent | Self::Failed
            ) | (
                Self::InstallingAgent,
                Self::InstallingAgent | Self::RegisteringAgent | Self::Failed
            ) | (
                Self::RegisteringAgent,
                Self::RegisteringAgent | Self::Connected | Self::Failed
            ) | (
                Self::Connected,
                Self::Connected | Self::Disabled | Self::Discovering
            ) | (Self::Failed, Self::Failed | Self::Discovering)
                | (Self::Disabled, Self::Disabled | Self::Discovering)
        )
    }

    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Requested | Self::Discovering | Self::InstallingAgent | Self::RegisteringAgent
        )
    }
}

/// State tracked while an existing LXC is enrolled into lxcup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Enrollment {
    pub id: EnrollmentId,
    pub container_id: ContainerId,
    pub state: EnrollmentState,
    pub idempotency_key: String,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Enrollment {
    pub fn new(
        container_id: ContainerId,
        idempotency_key: impl Into<String>,
    ) -> DomainResult<Self> {
        let idempotency_key = idempotency_key.into();
        if idempotency_key.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "enrollment idempotency key",
            });
        }
        if idempotency_key.chars().count() > 200 {
            return Err(DomainError::InvalidStateTransition(
                "enrollment idempotency key is too long",
            ));
        }

        let now = Utc::now();
        Ok(Self {
            id: EnrollmentId::new(),
            container_id,
            state: EnrollmentState::Requested,
            idempotency_key,
            failure_reason: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub const fn state(&self) -> EnrollmentState {
        self.state
    }

    pub fn transition_to(&mut self, next: EnrollmentState) -> DomainResult<()> {
        if !self.state.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition("enrollment state"));
        }
        self.state = next;
        if next != EnrollmentState::Failed {
            self.failure_reason = None;
        }
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        self.state = EnrollmentState::Failed;
        self.failure_reason = Some(reason.into());
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::{Enrollment, EnrollmentState};
    use crate::ContainerId;

    #[test]
    fn enrollment_follows_the_controlled_lifecycle() {
        let mut enrollment = Enrollment::new(ContainerId::new(101), "request-1").unwrap();

        assert_eq!(enrollment.state(), EnrollmentState::Requested);
        enrollment
            .transition_to(EnrollmentState::Discovering)
            .unwrap();
        enrollment
            .transition_to(EnrollmentState::InstallingAgent)
            .unwrap();
        enrollment
            .transition_to(EnrollmentState::RegisteringAgent)
            .unwrap();
        enrollment
            .transition_to(EnrollmentState::Connected)
            .unwrap();
        enrollment.transition_to(EnrollmentState::Disabled).unwrap();
    }

    #[test]
    fn failed_enrollment_can_be_retried_but_connected_cannot_skip_steps() {
        let mut failed = Enrollment::new(ContainerId::new(101), "request-1").unwrap();
        failed.fail("agent deployment failed");
        assert_eq!(failed.state(), EnrollmentState::Failed);
        failed.transition_to(EnrollmentState::Discovering).unwrap();

        let mut connected = Enrollment::new(ContainerId::new(102), "request-2").unwrap();
        connected
            .transition_to(EnrollmentState::Discovering)
            .unwrap();
        connected
            .transition_to(EnrollmentState::InstallingAgent)
            .unwrap();
        connected
            .transition_to(EnrollmentState::RegisteringAgent)
            .unwrap();
        connected.transition_to(EnrollmentState::Connected).unwrap();
        assert!(
            connected
                .transition_to(EnrollmentState::RegisteringAgent)
                .is_err()
        );
    }

    #[test]
    fn idempotency_key_is_required_and_bounded() {
        assert!(Enrollment::new(ContainerId::new(101), " ").is_err());
        assert!(Enrollment::new(ContainerId::new(101), "x".repeat(201)).is_err());
    }
}
