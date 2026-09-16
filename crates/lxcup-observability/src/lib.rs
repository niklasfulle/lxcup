//! Gemeinsame Initialisierung für strukturierte lxcup-Logs.

use tracing_subscriber::{EnvFilter, fmt};

/// Werttyp für Felder, die niemals im Klartext geloggt werden dürfen.
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Verpackt einen sensiblen Wert für eine sichere Log-Ausgabe.
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> std::fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl<T> std::fmt::Display for Redacted<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Initialisiert JSON-Logging mit konfigurierbarem `RUST_LOG`-Filter.
///
/// Mehrfache Initialisierung ist erlaubt und wird ignoriert, damit CLI und
/// Server ihre Initialisierung sicher aus einem gemeinsamen Einstiegspunkt
/// aufrufen können.
pub fn init(service: &'static str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .with_current_span(true)
        .with_span_list(true)
        .try_init();

    tracing::debug!(service, "observability initialized");
}

#[cfg(test)]
mod tests {
    #[test]
    fn observability_crate_is_available() {
        super::init("test");
    }

    #[test]
    fn redacted_values_never_expose_their_contents() {
        let value = super::Redacted::new("super-secret-token");

        assert_eq!(format!("{value:?}"), "[REDACTED]");
        assert_eq!(value.to_string(), "[REDACTED]");
    }
}
