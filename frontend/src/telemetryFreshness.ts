export const TELEMETRY_STALE_AFTER_MS = 2 * 60 * 1000;

export function isTelemetryStale(collectedAt: string | null | undefined, now = Date.now()) {
  if (!collectedAt) return true;
  const collectedAtMs = new Date(collectedAt).getTime();
  return !Number.isFinite(collectedAtMs) || now - collectedAtMs > TELEMETRY_STALE_AFTER_MS;
}
