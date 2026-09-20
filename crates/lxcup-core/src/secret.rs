use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ContainerId, DomainError, NodeId, SecretId, error::DomainResult};

/// Intended use of a stored credential. The kind is metadata only and never
/// contains the credential itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    ProxmoxApiToken,
    SshPrivateKey,
    SshPassword,
    AgentToken,
    Generic,
}

/// Resource scope a credential is allowed to access.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "id")]
pub enum SecretScope {
    Global,
    Node(NodeId),
    Container(ContainerId),
}

/// Safe-to-return metadata for a credential. It intentionally has no value
/// field, so API responses and logs cannot accidentally expose a secret.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecretMetadata {
    pub id: SecretId,
    pub name: String,
    pub kind: SecretKind,
    pub scope: SecretScope,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SecretMetadata {
    pub fn new(
        name: impl Into<String>,
        kind: SecretKind,
        scope: SecretScope,
    ) -> DomainResult<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "secret name",
            });
        }
        if name.chars().count() > 100 {
            return Err(DomainError::InvalidStateTransition(
                "secret name is too long",
            ));
        }

        let now = Utc::now();
        Ok(Self {
            id: SecretId::new(),
            name,
            kind,
            scope,
            created_at: now,
            updated_at: now,
        })
    }
}

/// Secret material that cannot be serialized or displayed in clear text.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::EmptyValue {
                field: "secret value",
            });
        }
        if value.chars().count() > 16_384 {
            return Err(DomainError::InvalidStateTransition(
                "secret value is too long",
            ));
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretKind, SecretMetadata, SecretScope, SecretValue};

    #[test]
    fn metadata_is_safe_to_serialize_without_secret_material() {
        let metadata = SecretMetadata::new(
            "proxmox-test",
            SecretKind::ProxmoxApiToken,
            SecretScope::Global,
        )
        .unwrap();
        let json = serde_json::to_string(&metadata).unwrap();

        assert!(json.contains("proxmox-test"));
        assert!(!json.contains("value"));
    }

    #[test]
    fn secret_value_is_redacted_and_validated() {
        let secret = SecretValue::new("token-value").unwrap();

        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(secret.to_string(), "[REDACTED]");
        assert_eq!(secret.expose(), "token-value");
        assert!(SecretValue::new("").is_err());
    }

    #[test]
    fn metadata_and_values_reject_unbounded_input() {
        assert!(SecretMetadata::new(" ", SecretKind::Generic, SecretScope::Global).is_err());
        assert!(
            SecretMetadata::new("x".repeat(101), SecretKind::Generic, SecretScope::Global).is_err()
        );
        assert!(SecretValue::new("x".repeat(16_385)).is_err());
    }
}
