/// Number of seconds shown in the live telemetry charts and API response.
pub const HISTORY_WINDOW_SECONDS: i64 = 10 * 60;

/// Persisted telemetry is retained independently from the shorter chart window.
pub const PERSISTED_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;
