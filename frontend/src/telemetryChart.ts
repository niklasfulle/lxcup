export const TELEMETRY_WINDOW_MS = 10 * 60_000;

export function telemetryXPosition(collectedAt: string, now: number, left: number, width: number) {
  const start = now - TELEMETRY_WINDOW_MS;
  const timestamp = new Date(collectedAt).getTime();
  const progress = Math.max(0, Math.min(1, (timestamp - start) / TELEMETRY_WINDOW_MS));
  return left + progress * width;
}
