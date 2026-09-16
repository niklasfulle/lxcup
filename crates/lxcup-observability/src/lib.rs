//! Gemeinsame Initialisierung für strukturierte lxcup-Logs.

use tracing_subscriber::{EnvFilter, fmt};

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
}
