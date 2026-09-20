use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{DomainError, EnvironmentId, SecretId};

/// Lifecycle state of a configured Proxmox environment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentStatus {
    Configured,
    Connected,
    Failed,
    Disabled,
}

impl EnvironmentStatus {
    pub const fn can_transition_to(self, next: Self) -> bool {
        match (self, next) {
            (Self::Configured, Self::Connected | Self::Failed | Self::Disabled) => true,
            (Self::Connected, Self::Connected | Self::Failed | Self::Disabled) => true,
            (Self::Failed, Self::Connected | Self::Failed | Self::Disabled) => true,
            (Self::Disabled, Self::Configured | Self::Disabled) => true,
            _ => false,
        }
    }
}

/// Stored, credential-free connection configuration for a Proxmox cluster.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProxmoxEnvironment {
    pub id: EnvironmentId,
    pub name: String,
    pub endpoint: String,
    pub api_secret_ref: SecretId,
    pub ca_secret_ref: Option<SecretId>,
    pub status: EnvironmentStatus,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_check_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProxmoxEnvironment {
    pub fn new(
        id: EnvironmentId,
        name: impl Into<String>,
        endpoint: impl Into<String>,
        api_secret_ref: SecretId,
        ca_secret_ref: Option<SecretId>,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let name = name.into().trim().to_owned();
        if name.is_empty() || name.len() > 100 {
            return Err(DomainError::EmptyValue {
                field: "environment name",
            });
        }

        let endpoint = endpoint.into().trim_end_matches('/').to_owned();
        let is_https = endpoint
            .strip_prefix("https://")
            .is_some_and(|host| !host.is_empty() && !host.chars().any(char::is_whitespace));
        if !is_https {
            return Err(DomainError::InvalidEnvironmentEndpoint);
        }

        Ok(Self {
            id,
            name,
            endpoint,
            api_secret_ref,
            ca_secret_ref,
            status: EnvironmentStatus::Configured,
            last_checked_at: None,
            last_check_error: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// Replaces connection metadata without accepting credential material.
    pub fn update_configuration(
        &mut self,
        name: impl Into<String>,
        endpoint: impl Into<String>,
        api_secret_ref: SecretId,
        ca_secret_ref: Option<SecretId>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        let replacement = Self::new(self.id, name, endpoint, api_secret_ref, ca_secret_ref, now)?;
        self.name = replacement.name;
        self.endpoint = replacement.endpoint;
        self.api_secret_ref = replacement.api_secret_ref;
        self.ca_secret_ref = replacement.ca_secret_ref;
        self.status = EnvironmentStatus::Configured;
        self.last_checked_at = None;
        self.last_check_error = None;
        self.updated_at = now;
        Ok(())
    }

    pub fn record_check_success(&mut self, checked_at: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition_to(EnvironmentStatus::Connected, checked_at)?;
        self.last_checked_at = Some(checked_at);
        self.last_check_error = None;
        Ok(())
    }

    pub fn record_check_failure(
        &mut self,
        checked_at: DateTime<Utc>,
        message: impl Into<String>,
    ) -> Result<(), DomainError> {
        self.transition_to(EnvironmentStatus::Failed, checked_at)?;
        self.last_checked_at = Some(checked_at);
        self.last_check_error = Some(message.into());
        Ok(())
    }

    pub fn transition_to(
        &mut self,
        next: EnvironmentStatus,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition(
                "proxmox environment lifecycle transition",
            ));
        }
        self.status = next;
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{EnvironmentId, SecretId};

    use super::{EnvironmentStatus, ProxmoxEnvironment};

    #[test]
    fn requires_https_and_normalizes_endpoint() {
        let now = Utc::now();
        assert!(
            ProxmoxEnvironment::new(
                EnvironmentId::new(),
                "pve",
                "http://pve.example.test",
                SecretId::new(),
                None,
                now,
            )
            .is_err()
        );

        let environment = ProxmoxEnvironment::new(
            EnvironmentId::new(),
            " pve ",
            "https://pve.example.test/",
            SecretId::new(),
            None,
            now,
        )
        .expect("valid environment");
        assert_eq!(environment.name, "pve");
        assert_eq!(environment.endpoint, "https://pve.example.test");
    }

    #[test]
    fn records_success_and_failure_without_secret_values() {
        let now = Utc::now();
        let mut environment = ProxmoxEnvironment::new(
            EnvironmentId::new(),
            "pve",
            "https://pve.example.test",
            SecretId::new(),
            Some(SecretId::new()),
            now,
        )
        .expect("valid environment");

        environment.record_check_success(now).expect("success");
        assert_eq!(environment.status, EnvironmentStatus::Connected);
        assert!(environment.last_check_error.is_none());

        environment
            .record_check_failure(now, "certificate rejected")
            .expect("failure");
        assert_eq!(environment.status, EnvironmentStatus::Failed);
        assert_eq!(
            environment.last_check_error.as_deref(),
            Some("certificate rejected")
        );

        let json = serde_json::to_string(&environment).expect("serialize");
        assert!(!json.contains("token"));
    }
}
