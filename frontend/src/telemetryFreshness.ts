export const TELEMETRY_STALE_AFTER_MS = 2 * 60 * 1000;

export function isTelemetryStale(collectedAt: string | null | undefined, now = Date.now()) {
  if (!collectedAt) return true;
  const collectedAtMs = new Date(collectedAt).getTime();
  return !Number.isFinite(collectedAtMs) || now - collectedAtMs > TELEMETRY_STALE_AFTER_MS;
}

export function telemetryAgeLabel(collectedAt: string | null | undefined, now = Date.now()) {
  if (!collectedAt) return "Zeitpunkt unbekannt";
  const collectedAtMs = new Date(collectedAt).getTime();
  if (!Number.isFinite(collectedAtMs)) return "Zeitpunkt unbekannt";
  const seconds = Math.max(0, Math.floor((now - collectedAtMs) / 1_000));
  if (seconds < 60) return `vor ${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `vor ${minutes} Min.`;
  return `vor ${Math.floor(minutes / 60)} Std.`;
}
