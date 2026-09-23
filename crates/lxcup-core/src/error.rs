use serde::Serialize;
use thiserror::Error;

/// Stable error categories shared by API responses, logs and metrics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    Unauthorized,
    Forbidden,
    Conflict,
    DependencyUnavailable,
    Timeout,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Conflict => "conflict",
            Self::DependencyUnavailable => "dependency_unavailable",
            Self::Timeout => "timeout",
            Self::Internal => "internal_error",
        }
    }
}

/// Bounded retry policy used at external-system boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u8,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl RetryPolicy {
    pub const fn conservative() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 2_000,
        }
    }

    pub const fn delay_ms(self, retry_number: u8) -> u64 {
        let shift = if retry_number > 6 { 6 } else { retry_number } as u32;
        let value = self.initial_backoff_ms.saturating_mul(1_u64 << shift);
        if value > self.max_backoff_ms {
            self.max_backoff_ms
        } else {
            value
        }
    }

    pub const fn allows_retry(self, attempts_completed: u8) -> bool {
        attempts_completed.saturating_add(1) < self.max_attempts
    }
}

/// Fehler, die auf ungültige Domainwerte oder Zustände hinweisen.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    /// Ein erforderlicher Wert ist leer.
    #[error("{field} must not be empty")]
    EmptyValue { field: &'static str },

    /// Ein Paketname enthält Leerzeichen und ist daher nicht kanonisch.
    #[error("package name must not contain whitespace")]
    InvalidPackageName,

    /// Eine Endpoint-URL ist nicht sicher oder nicht kanonisch.
    #[error("environment endpoint must be a valid https URL without whitespace")]
    InvalidEnvironmentEndpoint,

    /// Eine angeforderte Zustandsänderung ist nicht erlaubt.
    #[error("invalid domain state transition: {0}")]
    InvalidStateTransition(&'static str),

    /// Die Rolle darf die angeforderte Operation nicht ausführen.
    #[error("permission denied")]
    PermissionDenied,

    /// Eine verändernde oder destruktive Aktion benötigt eine Bestätigung.
    #[error("explicit confirmation is required")]
    ConfirmationRequired,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_snake_case_values() {
        let codes = [
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::Unauthorized, "unauthorized"),
            (ErrorCode::Forbidden, "forbidden"),
            (ErrorCode::Conflict, "conflict"),
            (ErrorCode::DependencyUnavailable, "dependency_unavailable"),
            (ErrorCode::Timeout, "timeout"),
            (ErrorCode::Internal, "internal_error"),
        ];
        for (code, expected) in codes {
            assert_eq!(code.as_str(), expected);
            assert!(!serde_json::to_string(&code).unwrap().is_empty());
        }
    }

    #[test]
    fn retry_policy_is_bounded_and_allows_only_configured_attempts() {
        let policy = RetryPolicy::conservative();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.delay_ms(0), 100);
        assert_eq!(policy.delay_ms(1), 200);
        assert_eq!(policy.delay_ms(6), 2_000);
        assert_eq!(policy.delay_ms(255), 2_000);
        assert!(policy.allows_retry(0));
        assert!(policy.allows_retry(1));
        assert!(!policy.allows_retry(2));
        assert!(!policy.allows_retry(u8::MAX));
    }

    #[test]
    fn infrastructure_and_domain_errors_expose_safe_categories() {
        let domain = LxcupError::from(DomainError::PermissionDenied);
        assert_eq!(domain.code(), "domain_error");
        assert_eq!(
            domain.public_message(),
            "The requested domain operation is not valid."
        );

        let source = std::io::Error::other("secret detail");
        let infrastructure = InfrastructureError::new("connect", source);
        assert_eq!(infrastructure.operation(), "connect");
        let error = LxcupError::from(infrastructure);
        assert_eq!(error.code(), "infrastructure_error");
        assert_eq!(
            error.public_message(),
            "A required infrastructure operation failed."
        );
        assert!(!error.to_string().contains("secret detail"));
    }

    #[test]
    fn domain_error_messages_remain_actionable() {
        assert_eq!(
            DomainError::EmptyValue { field: "name" }.to_string(),
            "name must not be empty"
        );
        assert_eq!(
            DomainError::InvalidPackageName.to_string(),
            "package name must not contain whitespace"
        );
        assert_eq!(
            DomainError::InvalidEnvironmentEndpoint.to_string(),
            "environment endpoint must be a valid https URL without whitespace"
        );
        assert_eq!(
            DomainError::InvalidStateTransition("queued").to_string(),
            "invalid domain state transition: queued"
        );
        assert_eq!(
            DomainError::ConfirmationRequired.to_string(),
            "explicit confirmation is required"
        );
    }
}
