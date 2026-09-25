export function dockerDiscoveryFailureLabel(value: string) {
  const normalized = value.toLowerCase();
  if (normalized.includes("agent_unavailable") || normalized.includes("agent not registered")) {
    return "Kein Agent für diesen LXC registriert.";
  }
  if (normalized.includes("docker_unavailable") || normalized.includes("docker ist nicht verfügbar")) {
    return "Docker ist auf diesem Host nicht verfügbar.";
  }
  return value;
}
