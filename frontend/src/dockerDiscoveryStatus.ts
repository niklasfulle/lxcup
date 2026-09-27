export function dockerDiscoveryFailureLabel(value: string) {
  const normalized = value.toLowerCase();
  if (normalized.includes("agent_unavailable") || normalized.includes("agent not registered")) {
    return "Kein Agent für diesen LXC registriert.";
  }
  if (normalized.includes("docker_permission_denied") || normalized.includes("keine berechtigung für den docker-socket")) {
    return "Docker ist installiert, aber lxcup-agent darf nicht auf den Docker-Socket zugreifen. Füge lxcup-agent der Gruppe docker hinzu und starte den Agent-Dienst neu. Die Gruppe docker gewährt praktisch root-äquivalente Rechte.";
  }
  if (normalized.includes("docker_cli_unavailable")) {
    return "Die Docker-CLI ist für lxcup-agent nicht verfügbar. Prüfe, ob Docker installiert ist und im Systemdienst-PATH liegt.";
  }
  if (normalized.includes("docker_unavailable") || normalized.includes("docker ist nicht verfügbar")) {
    return "Docker ist auf diesem Host nicht verfügbar.";
  }
  return value;
}
