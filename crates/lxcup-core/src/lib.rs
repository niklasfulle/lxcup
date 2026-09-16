//! Domain- und Geschäftslogik von lxcup.
//!
//! Dieses Crate bleibt unabhängig von HTTP, PostgreSQL und konkreten
//! Proxmox- oder Betriebssystem-Adaptern.

pub mod error {
    //! Gemeinsame Fehlerbasis für die Domain.

    use thiserror::Error;

    /// Fehler, die auf ungültige Domainzustände hinweisen.
    #[derive(Debug, Error, PartialEq, Eq)]
    pub enum DomainError {
        /// Eine angeforderte Zustandsänderung ist nicht erlaubt.
        #[error("invalid domain state transition: {0}")]
        InvalidStateTransition(&'static str),
    }

    /// Einheitlicher Result-Typ für Domainoperationen.
    pub type DomainResult<T> = Result<T, DomainError>;
}

/// Version des Core-Crates.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn exposes_package_version() {
        assert_eq!(VERSION, "0.1.0");
    }
}
