use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AgentRegistrationId, ContainerId, DomainError, SecretId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Connected,
    Degraded,
    Unreachable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRegistration {
    pub id: AgentRegistrationId,
    pub container_id: ContainerId,
    pub agent_id: String,
    pub endpoint: String,
    pub secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
    pub state: AgentConnectionState,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentRegistration {
    pub fn new(
        container_id: ContainerId,
        agent_id: impl Into<String>,
        endpoint: impl Into<String>,
        secret_ref: SecretId,
        ca_secret_ref: Option<SecretId>,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let agent_id = agent_id.into().trim().to_owned();
        let endpoint = endpoint.into().trim_end_matches('/').to_owned();
        if agent_id.is_empty() || agent_id.len() > 200 {
            return Err(DomainError::EmptyValue { field: "agent id" });
        }
        if !(endpoint.starts_with("https://")
            || endpoint.starts_with("http://127.0.0.1")
            || endpoint.starts_with("http://localhost"))
        {
            return Err(DomainError::InvalidEnvironmentEndpoint);
        }
        Ok(Self {
            id: AgentRegistrationId::new(),
            container_id,
            agent_id,
            endpoint,
            secret_ref,
            ca_secret_ref,
            state: AgentConnectionState::Connected,
            last_checked_at: Some(now),
            last_error: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn record_health(&mut self, healthy: bool, now: DateTime<Utc>, error: Option<String>) {
        self.state = if healthy {
            AgentConnectionState::Connected
        } else {
            AgentConnectionState::Degraded
        };
        self.last_checked_at = Some(now);
        self.last_error = error;
        self.updated_at = now;
    }

    pub fn record_unreachable(&mut self, now: DateTime<Utc>, error: impl Into<String>) {
        self.state = AgentConnectionState::Unreachable;
        self.last_checked_at = Some(now);
        self.last_error = Some(error.into());
        self.updated_at = now;
    }
}

#[cfg(test)]
mod tests {
    use crate::{AgentConnectionState, AgentRegistration, ContainerId, SecretId};
    use chrono::Utc;

    #[test]
    fn registration_accepts_https_and_local_development_endpoints_only() {
        let now = Utc::now();
        assert!(
            AgentRegistration::new(
                ContainerId::new(101),
                "agent-1",
                "https://agent",
                SecretId::new(),
                None,
                now
            )
            .is_ok()
        );
        assert!(
            AgentRegistration::new(
                ContainerId::new(101),
                "agent-1",
                "http://agent",
                SecretId::new(),
                None,
                now
            )
            .is_err()
        );
        assert!(
            AgentRegistration::new(
                ContainerId::new(101),
                "agent-1",
                "http://127.0.0.1:8090",
                SecretId::new(),
                None,
                now
            )
            .is_ok()
        );
    }

    #[test]
    fn health_reconciliation_distinguishes_degraded_and_unreachable() {
        let now = Utc::now();
        let mut registration = AgentRegistration::new(
            ContainerId::new(101),
            "agent-1",
            "https://agent",
            SecretId::new(),
            None,
            now,
        )
        .unwrap();
        registration.record_health(false, now, Some("agent reported unhealthy".to_owned()));
        assert_eq!(registration.state, AgentConnectionState::Degraded);
        registration.record_unreachable(now, "timeout");
        assert_eq!(registration.state, AgentConnectionState::Unreachable);
    }
}
