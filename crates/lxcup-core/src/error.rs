use thiserror::Error;

/// Fehler, die auf ungültige Domainwerte oder Zustände hinweisen.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    /// Ein erforderlicher Wert ist leer.
    #[error("{field} must not be empty")]
    EmptyValue { field: &'static str },

    /// Ein Paketname enthält Leerzeichen und ist daher nicht kanonisch.
    #[error("package name must not contain whitespace")]
    InvalidPackageName,

    /// Eine angeforderte Zustandsänderung ist nicht erlaubt.
    #[error("invalid domain state transition: {0}")]
    InvalidStateTransition(&'static str),
}

/// Einheitlicher Result-Typ für Domainoperationen.
pub type DomainResult<T> = Result<T, DomainError>;

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

/// Einheitlicher Result-Typ für Anwendungsgrenzen.
pub type LxcupResult<T> = Result<T, LxcupError>;
