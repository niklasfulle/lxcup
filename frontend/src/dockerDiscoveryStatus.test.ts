import { describe, expect, it } from "vitest";
import { dockerDiscoveryFailureLabel } from "./dockerDiscoveryStatus";

describe("docker discovery failure labels", () => {
  it("explains missing agents and unavailable Docker without exposing opaque codes", () => {
    expect(dockerDiscoveryFailureLabel("agent_unavailable")).toBe("Kein Agent für diesen LXC registriert.");
    expect(dockerDiscoveryFailureLabel("Docker ist nicht verfügbar; bestehende Funde bleiben unverändert")).toBe("Docker ist auf diesem Host nicht verfügbar.");
  });

  it("preserves an explanatory message for other discovery errors", () => {
    expect(dockerDiscoveryFailureLabel("Docker-Inventar konnte nicht gelesen werden")).toBe("Docker-Inventar konnte nicht gelesen werden");
  });
});
