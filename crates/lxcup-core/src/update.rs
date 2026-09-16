use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// Kanonischer Name eines Betriebssystempakets.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PackageName(String);

impl PackageName {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "package name",
            });
        }
        if value.chars().any(char::is_whitespace) {
            return Err(DomainError::InvalidPackageName);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Von einem Paketmanager gemeldete Paketversion.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PackageVersion(String);

impl PackageVersion {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::EmptyValue {
                field: "package version",
            });
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Klassifizierung eines verfügbaren Updates.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum UpdateClassification {
    Security,
    Normal,
    Unknown,
}

/// Ein beim Scan erkanntes verfügbares Paketupdate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AvailableUpdate {
    pub package: PackageName,
    pub installed_version: PackageVersion,
    pub candidate_version: PackageVersion,
    pub classification: UpdateClassification,
    pub held: bool,
}

#[cfg(test)]
mod tests {
    use super::{PackageName, PackageVersion};

    #[test]
    fn accepts_architecture_qualified_package_names() {
        let package = PackageName::new("libc6:amd64").unwrap();

        assert_eq!(package.as_str(), "libc6:amd64");
    }

    #[test]
    fn rejects_whitespace_in_package_names() {
        assert!(PackageName::new("openssl dev").is_err());
    }

    #[test]
    fn accepts_non_empty_versions() {
        let version = PackageVersion::new("3.5.1-1").unwrap();

        assert_eq!(version.as_str(), "3.5.1-1");
    }
}
