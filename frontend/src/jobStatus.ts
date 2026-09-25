const labels: Record<string, string> = {
  queued: "Wartet",
  checking: "Prüft",
  planned: "Geplant",
  applying: "Läuft",
  succeeded: "Erfolgreich",
  completed: "Erfolgreich",
  failed: "Fehlgeschlagen",
  aborted: "Abgebrochen",
  cancelled: "Abgebrochen",
  reconcile_required: "Abgleich erforderlich",
};

export function jobStatusLabel(status: string) {
  const label = labels[status];
  return typeof label === "string" ? label : status;
}

export function jobStatusBadgeClass(status: string) {
  if (status === "succeeded" || status === "completed") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (status === "failed" || status === "aborted" || status === "cancelled" || status === "reconcile_required") return "bg-[var(--error-soft)] text-[var(--error)]";
  if (status === "planned") return "bg-[var(--warning-soft)] text-[var(--warning)]";
  if (status === "queued" || status === "checking" || status === "applying") return "bg-[var(--primary-soft)] text-lxcup-primary";
  return "bg-[var(--paper-muted)] text-[var(--muted)]";
}
