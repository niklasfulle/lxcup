import { describe, expect, it } from "vitest";
import { TELEMETRY_WINDOW_MS, telemetryXPosition } from "./telemetryChart";

describe("telemetry chart window", () => {
  it("shows ten minutes and places samples proportionally across the chart", () => {
    const now = Date.parse("2026-09-26T12:00:00Z");

    expect(TELEMETRY_WINDOW_MS).toBe(600_000);
    expect(telemetryXPosition(new Date(now - TELEMETRY_WINDOW_MS).toISOString(), now, 10, 580)).toBe(10);
    expect(telemetryXPosition(new Date(now - TELEMETRY_WINDOW_MS / 2).toISOString(), now, 10, 580)).toBe(300);
    expect(telemetryXPosition(new Date(now).toISOString(), now, 10, 580)).toBe(590);
  });

  it("keeps out-of-window samples clamped to the chart edges", () => {
    const now = Date.parse("2026-09-26T12:00:00Z");

    expect(telemetryXPosition(new Date(now - TELEMETRY_WINDOW_MS * 2).toISOString(), now, 10, 580)).toBe(10);
    expect(telemetryXPosition(new Date(now + TELEMETRY_WINDOW_MS).toISOString(), now, 10, 580)).toBe(590);
  });
});
