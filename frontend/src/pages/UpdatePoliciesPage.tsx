import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createUpdatePolicy, deleteUpdatePolicy } from "../api";
import { queryKeys, useTargets, useUpdatePolicies } from "../queries";
import { cn } from "../classnames";

const panelClass = "border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)] shadow-sm";
const fieldClass = "grid content-start min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]";
const inputClass = "block h-10 w-full rounded-sm border border-[var(--line)] bg-[var(--paper)] px-3 py-2 text-sm font-normal leading-5 text-[var(--ink)] transition-colors focus:border-[var(--primary)] focus:ring-2 focus:ring-[var(--primary)]/20";
const primaryButtonClass = "inline-flex min-h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 py-2 text-sm font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50";
const standardPolicyId = "lxcup-standard-all-packages";

export function UpdatePoliciesPage() {
  const targets = useTargets();
  const policies = useUpdatePolicies();
  const queryClient = useQueryClient();
  const [id, setId] = useState("");
  const [targetId, setTargetId] = useState("");
  const [allowedPackages, setAllowedPackages] = useState("");
  const [maximumRisk, setMaximumRisk] = useState<"low" | "medium" | "high">("high");
  const [maintenanceStart, setMaintenanceStart] = useState("00:00");
  const [maintenanceEnd, setMaintenanceEnd] = useState("23:59");
  const targetList = targets.data ?? [];
  const policyList = policies.data ?? [];

  const mutation = useMutation({
    mutationFn: () => createUpdatePolicy({
      id,
      allowed_targets: [targetId],
      allowed_packages: allowedPackages.split(",").map((item) => item.trim()).filter(Boolean),
      maintenance_start_minute: toMinute(maintenanceStart),
      maintenance_end_minute: toMinute(maintenanceEnd),
      timezone: "UTC",
      maximum_risk: maximumRisk,
      enabled: true,
    }),
    onSuccess: () => {
      setId("");
      setAllowedPackages("");
      void queryClient.invalidateQueries({ queryKey: queryKeys.updatePolicies });
    },
  });
  const deleteMutation = useMutation({
    mutationFn: (policyId: string) => deleteUpdatePolicy(policyId),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: queryKeys.updatePolicies }),
  });

  function confirmPolicyDeletion(policyId: string) {
    const message = `Policy „${policyId}“ wirklich löschen? Bereits erstellte Pläne mit dieser Policy können danach nicht mehr angewendet werden.`;
    if (window.confirm(message)) deleteMutation.mutate(policyId);
  }

  return <div className="grid gap-5">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4 max-[720px]:items-start">
      <div>
        <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Automatisierung · Sicherheit</p>
        <h1 className="mb-1">Update-Policies</h1>
        <p className="mb-0 text-sm text-[var(--muted)]">Lege fest, welche Pakete auf welchen Zielen und in welchem Wartungsfenster aktualisiert werden dürfen.</p>
      </div>
      <div className="flex shrink-0 items-center" aria-label="Policyübersicht">
        <span className="inline-flex min-h-9 items-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs text-[var(--muted)]"><strong className="mr-1 text-[var(--ink)]">{policyList.filter((policy) => policy.enabled).length}</strong>aktive Policies</span>
      </div>
    </header>

    <section className={panelClass} aria-labelledby="policy-create-title">
      <div className="mb-5 border-b border-[var(--line)] pb-4">
        <h2 id="policy-create-title" className="mb-1">Policy erstellen</h2>
        <p className="mb-0 text-sm text-[var(--muted)]">Policies sind Freigaben für Paketupdate-Jobs. Das Wartungsfenster wird in UTC ausgewertet.</p>
      </div>
      <form className="grid gap-4" onSubmit={(event) => { event.preventDefault(); if (id.trim() && targetId) mutation.mutate(); }}>
        <div className="grid gap-x-4 gap-y-4 sm:grid-cols-2 xl:grid-cols-3">
          <label className={fieldClass}><span>Name</span><input className={inputClass} value={id} onChange={(event) => setId(event.target.value)} placeholder="security-updates" required /></label>
          <label className={fieldClass}><span>Freigegebenes Ziel</span><select className={inputClass} value={targetId} onChange={(event) => setTargetId(event.target.value)} required><option value="">Ziel auswählen</option>{targetList.map((target) => <option key={target.id} value={target.id}>{target.name}</option>)}</select></label>
          <label className={fieldClass}><span>Maximales Risiko</span><select className={inputClass} value={maximumRisk} onChange={(event) => setMaximumRisk(event.target.value as typeof maximumRisk)}><option value="low">Niedrig</option><option value="medium">Mittel</option><option value="high">Hoch</option></select></label>
          <label className={fieldClass}><span>Erlaubte Pakete</span><input className={inputClass} value={allowedPackages} onChange={(event) => setAllowedPackages(event.target.value)} placeholder="curl, openssl" /><span className="font-normal">Kommagetrennt; leer lassen, um alle Pakete zu erlauben.</span></label>
          <div className="grid gap-3 sm:grid-cols-2 sm:col-span-2">
            <label className={fieldClass}><span>Wartungsfenster von</span><input className={inputClass} type="time" value={maintenanceStart} onChange={(event) => setMaintenanceStart(event.target.value)} /></label>
            <label className={fieldClass}><span>bis</span><input className={inputClass} type="time" value={maintenanceEnd} onChange={(event) => setMaintenanceEnd(event.target.value)} /></label>
            <p className="m-0 border-l-2 border-[var(--primary)] bg-[var(--primary-soft)] px-3 py-2 text-xs text-[var(--muted)] sm:col-span-2"><strong className="text-[var(--ink)]">Zeitzone: UTC</strong><span className="mt-1 block">Das Wartungsfenster wird in UTC ausgewertet.</span></p>
          </div>
        </div>
        {targets.error ? <p className="m-0 font-semibold text-[var(--error)]" role="alert">Ziele konnten nicht geladen werden: {targets.error.message}</p> : null}
        {mutation.error ? <p className="m-0 font-semibold text-[var(--error)]" role="alert">{mutation.error.message}</p> : null}
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--line)] pt-4">
          <p className="m-0 text-xs text-[var(--muted)]">{targetList.length ? `${targetList.length} Ziele verfügbar` : "Lege zuerst ein Ziel an, für das du Updates freigeben möchtest."}</p>
          <button className={primaryButtonClass} type="submit" disabled={!id.trim() || !targetId || mutation.isPending || !targetList.length}>{mutation.isPending ? "Wird gespeichert…" : "Policy speichern"}<span aria-hidden="true">→</span></button>
        </div>
      </form>
    </section>

    <section className={panelClass} aria-labelledby="policy-list-title">
      <div className="mb-4 flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-4">
        <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Freigaben</p><h2 id="policy-list-title" className="mb-1">Vorhandene Policies</h2><p className="mb-0 text-sm text-[var(--muted)]">Nur aktive Policies können beim Erstellen eines Paketupdate-Zeitplans ausgewählt werden.</p></div>
        <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-1.5 text-xs font-semibold text-[var(--muted)]">{policyList.length} gesamt</span>
      </div>
      {deleteMutation.error ? <p className="mb-3 font-semibold text-[var(--error)]" role="alert">Policy konnte nicht gelöscht werden: {deleteMutation.error.message}</p> : null}
      <PolicyContent policies={policies} targets={targetList} onDeletePolicy={confirmPolicyDeletion} deletingPolicyId={deleteMutation.isPending ? deleteMutation.variables : undefined} />
      <div className="mt-4 border-t border-[var(--line)] pt-4"><Link className="text-sm font-semibold text-lxcup-primary hover:underline" to="/schedules">← Zu den Zeitplänen</Link></div>
    </section>
  </div>;
}

function PolicyContent({ policies, targets, onDeletePolicy, deletingPolicyId }: Readonly<{ policies: ReturnType<typeof useUpdatePolicies>; targets: NonNullable<ReturnType<typeof useTargets>["data"]>; onDeletePolicy: (policyId: string) => void; deletingPolicyId: string | undefined }>) {
  if (policies.isLoading) return <p className="m-0 py-8 text-center text-sm text-[var(--muted)]">Policies werden geladen…</p>;
  if (policies.error) return <p className="m-0 py-4 font-semibold text-[var(--error)]" role="alert">{policies.error.message}</p>;
  if (!policies.data?.length) return <div className="grid min-h-36 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-4 py-8 text-center"><div><strong className="block text-sm">Noch keine Update-Policies</strong><span className="mt-1 block text-xs text-[var(--muted)]">Erstelle eine Freigabe, bevor du einen Paketupdate-Zeitplan anlegst.</span></div></div>;
  return <div className="grid gap-3 md:grid-cols-2">
    {policies.data.map((policy) => <article key={policy.id} className="grid content-start gap-4 border border-[var(--line)] bg-[var(--paper)] p-4">
      <header className="flex flex-wrap items-start justify-between gap-3"><div><h3 className="mb-1 break-all text-base font-semibold">{policy.id}</h3><p className="mb-0 text-sm text-[var(--muted)]">{policy.allowed_targets.map((targetId) => targets.find((target) => target.id === targetId)?.name ?? targetId).join(", ")}</p></div><span className={cn("inline-flex items-center gap-1.5 px-2 py-1 text-[10px] font-bold uppercase tracking-wide", policy.enabled ? "bg-[var(--success-soft)] text-[var(--success)]" : "bg-[var(--paper-muted)] text-[var(--muted)]")}><span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true" />{policy.enabled ? "Aktiv" : "Pausiert"}</span></header>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-3 border-t border-[var(--line)] pt-3 text-xs"><div><dt className="text-[var(--muted)]">Pakete</dt><dd className="mt-1 font-semibold">{policy.allowed_packages.join(", ") || "Alle Pakete"}</dd></div><div><dt className="text-[var(--muted)]">Maximales Risiko</dt><dd className="mt-1 font-semibold">{riskLabel(policy.maximum_risk)}</dd></div><div><dt className="text-[var(--muted)]">Wartungsfenster (UTC)</dt><dd className="mt-1 font-semibold">{formatMinute(policy.maintenance_start_minute)}–{formatMinute(policy.maintenance_end_minute)}</dd></div><div><dt className="text-[var(--muted)]">Zeitzone</dt><dd className="mt-1 font-semibold">{policy.timezone}</dd></div></dl>
      <footer className="flex justify-end border-t border-[var(--line)] pt-3">{policy.id === standardPolicyId ? <span className="text-xs text-[var(--muted)]">System-Policy · kann nicht gelöscht werden</span> : <button className="inline-flex min-h-9 items-center justify-center border border-[var(--error)] px-3 py-1.5 text-xs font-semibold text-[var(--error)] transition-colors hover:bg-[var(--error-soft)] disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={deletingPolicyId !== undefined} onClick={() => onDeletePolicy(policy.id)}>{deletingPolicyId === policy.id ? "Wird gelöscht…" : "Policy löschen"}</button>}</footer>
    </article>)}
  </div>;
}

function toMinute(time: string) {
  const [hours, minutes] = time.split(":").map(Number);
  return hours * 60 + minutes;
}

function formatMinute(value: number) {
  const hours = Math.floor(value / 60).toString().padStart(2, "0");
  const minutes = (value % 60).toString().padStart(2, "0");
  return `${hours}:${minutes}`;
}

function riskLabel(risk: string) {
  const labels: Record<string, string> = { low: "Niedrig", medium: "Mittel", high: "Hoch" };
  return labels[risk] ?? risk;
}
