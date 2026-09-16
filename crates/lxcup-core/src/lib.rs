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

    /// Fehler aus externen Systemen oder technischen Adaptern.
    #[derive(Debug, Error)]
    #[error("infrastructure operation failed: {operation}")]
    pub struct InfrastructureError {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    }

    impl InfrastructureError {
        /// Erzeugt einen Infrastrukturfehler mit einer technischen Fehlerkette.
        pub fn new(
            operation: &'static str,
            source: impl std::error::Error + Send + Sync + 'static,
        ) -> Self {
            Self {
                operation,
                source: Box::new(source),
            }
        }

        /// Liefert die technische Operation für eine stabile API-Kategorie.
        pub const fn operation(&self) -> &'static str {
            self.operation
        }
    }

    /// Gemeinsamer Fehler für Domain- und Infrastrukturgrenzen.
    #[derive(Debug, Error)]
    pub enum LxcupError {
        #[error(transparent)]
        Domain(#[from] DomainError),
        #[error(transparent)]
        Infrastructure(#[from] InfrastructureError),
    }

    impl LxcupError {
        /// Stabile Kategorie für API-DTOs, Metriken und Audit-Ereignisse.
        pub const fn code(&self) -> &'static str {
            match self {
                Self::Domain(_) => "domain_error",
                Self::Infrastructure(_) => "infrastructure_error",
            }
        }

        /// Sichere, technische Details freie Meldung für API-Clients.
        pub const fn public_message(&self) -> &'static str {
            match self {
                Self::Domain(_) => "The requested domain operation is not valid.",
                Self::Infrastructure(_) => "A required infrastructure operation failed.",
            }
        }
    }

    /// Einheitlicher Result-Typ für Domainoperationen.
    pub type DomainResult<T> = Result<T, DomainError>;

    /// Einheitlicher Result-Typ für Anwendungsgrenzen.
    pub type LxcupResult<T> = Result<T, LxcupError>;
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
