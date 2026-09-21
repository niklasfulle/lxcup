use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{DomainError, SecretId, TargetId};

/// The managed machine abstraction used uniformly for LXC, Linux and Windows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Lxc,
    LinuxServer,
    WindowsServer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetTransport {
    Ssh,
    Winrm,
}

impl TargetTransport {
    pub const fn supports(self, kind: TargetKind) -> bool {
        matches!(
            (self, kind),
            (Self::Ssh, TargetKind::Lxc | TargetKind::LinuxServer)
                | (Self::Winrm, TargetKind::WindowsServer)
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Pending,
    Managed,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Target {
    pub id: TargetId,
    pub name: String,
    pub kind: TargetKind,
    pub address: String,
    pub transport: TargetTransport,
    pub credential_secret_ref: SecretId,
    pub agent_secret_ref: SecretId,
    pub state: TargetState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Target {
    pub fn new(
        name: impl Into<String>,
        kind: TargetKind,
        address: impl Into<String>,
        transport: TargetTransport,
        credential_secret_ref: SecretId,
        agent_secret_ref: SecretId,
    ) -> Result<Self, DomainError> {
        let name = name.into().trim().to_owned();
        let address = address.into().trim().to_owned();
        if name.is_empty() {
            return Err(DomainError::EmptyValue {
                field: "target name",
            });
        }
        if address.is_empty() {
            return Err(DomainError::EmptyValue {
                field: "target address",
            });
        }
        if !transport.supports(kind) {
            return Err(DomainError::InvalidStateTransition(
                "target kind does not support transport",
            ));
        }
        let now = Utc::now();
        Ok(Self {
            id: TargetId::new(),
            name,
            kind,
            address,
            transport,
            credential_secret_ref,
            agent_secret_ref,
            state: TargetState::Pending,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn mark_managed(&mut self) {
        self.state = TargetState::Managed;
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_accept_only_their_supported_transport() {
        assert!(
            Target::new(
                "api",
                TargetKind::LinuxServer,
                "10.0.0.5",
                TargetTransport::Ssh,
                SecretId::new(),
                SecretId::new()
            )
            .is_ok()
        );
        assert!(
            Target::new(
                "win",
                TargetKind::WindowsServer,
                "10.0.0.6",
                TargetTransport::Winrm,
                SecretId::new(),
                SecretId::new()
            )
            .is_ok()
        );
        assert!(
            Target::new(
                "win",
                TargetKind::WindowsServer,
                "10.0.0.6",
                TargetTransport::Ssh,
                SecretId::new(),
                SecretId::new()
            )
            .is_err()
        );
    }
}
