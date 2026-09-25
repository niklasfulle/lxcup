import { describe, expect, it } from "vitest";
import { isTelemetryStale, telemetryAgeLabel, TELEMETRY_STALE_AFTER_MS } from "./telemetryFreshness";

describe("telemetry freshness", () => {
  const now = Date.parse("2026-09-25T12:00:00Z");

  it("keeps samples fresh through the two-minute grace period", () => {
    const collectedAt = new Date(now - TELEMETRY_STALE_AFTER_MS).toISOString();
    expect(isTelemetryStale(collectedAt, now)).toBe(false);
  });

  it("marks samples stale after the grace period", () => {
    const collectedAt = new Date(now - TELEMETRY_STALE_AFTER_MS - 1).toISOString();
    expect(isTelemetryStale(collectedAt, now)).toBe(true);
  });

  it("treats missing or invalid timestamps as stale", () => {
    expect(isTelemetryStale(null, now)).toBe(true);
    expect(isTelemetryStale("not-a-date", now)).toBe(true);
  });

  it("formats an age that advances with the supplied clock", () => {
    const collectedAt = new Date(now - 30_000).toISOString();
    expect(telemetryAgeLabel(collectedAt, now)).toBe("vor 30 s");
    expect(telemetryAgeLabel(collectedAt, now + 90_000)).toBe("vor 2 Min.");
    expect(telemetryAgeLabel("invalid", now)).toBe("Zeitpunkt unbekannt");
  });
});
