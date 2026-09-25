import { describe, expect, it } from "vitest";
import { jobStatusBadgeClass, jobStatusLabel } from "./jobStatus";

describe("job status labels", () => {
  it.each([
    ["queued", "Wartet"],
    ["checking", "Prüft"],
    ["planned", "Geplant"],
    ["applying", "Läuft"],
    ["succeeded", "Erfolgreich"],
    ["failed", "Fehlgeschlagen"],
    ["aborted", "Abgebrochen"],
    ["reconcile_required", "Abgleich erforderlich"],
  ])("formats %s consistently as %s", (status, label) => {
    expect(jobStatusLabel(status)).toBe(label);
    expect(jobStatusBadgeClass(status)).not.toBe("");
  });
});
