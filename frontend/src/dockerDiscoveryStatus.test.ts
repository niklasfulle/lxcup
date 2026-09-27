import { describe, expect, it } from "vitest";
import { dockerDiscoveryFailureLabel } from "./dockerDiscoveryStatus";

describe("docker discovery failure labels", () => {
  it("explains missing agents and unavailable Docker without exposing opaque codes", () => {
    expect(dockerDiscoveryFailureLabel("agent_unavailable")).toBe("Kein Agent für diesen LXC registriert.");
    expect(dockerDiscoveryFailureLabel("Docker ist nicht verfügbar; bestehende Funde bleiben unverändert")).toBe("Docker ist auf diesem Host nicht verfügbar.");
  });

  it("explains Docker socket permission failures and the required agent group", () => {
    expect(dockerDiscoveryFailureLabel("docker_permission_denied")).toContain("lxcup-agent darf nicht auf den Docker-Socket zugreifen");
    expect(dockerDiscoveryFailureLabel("docker_permission_denied")).toContain("Gruppe docker");
  });

  it("distinguishes a missing Docker CLI from socket permission failures", () => {
    expect(dockerDiscoveryFailureLabel("docker_cli_unavailable")).toContain("Docker-CLI ist für lxcup-agent nicht verfügbar");
  });

  it("preserves an explanatory message for other discovery errors", () => {
    expect(dockerDiscoveryFailureLabel("Docker-Inventar konnte nicht gelesen werden")).toBe("Docker-Inventar konnte nicht gelesen werden");
  });
});
