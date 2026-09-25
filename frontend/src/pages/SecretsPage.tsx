import { Link } from "react-router-dom";
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createSecret,
  deleteSecret,
  listSecretAudit,
  listSecrets,
  revokeSecret,
  rotateSecret,
  ApiError,
  type SecretKind,
} from "../api";

const secretKinds: Array<{ value: SecretKind; label: string }> = [
  { value: "agent_token", label: "Agent-Token" },
  { value: "ssh_private_key", label: "SSH Private Key" },
  { value: "ssh_password", label: "SSH Passwort" },
  { value: "ssh_known_hosts", label: "SSH Known Hosts" },
  { value: "generic", label: "Allgemein" },
];

const primaryButton = "inline-flex min-h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 py-2 text-sm font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50";
const secondaryButton = "inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50";
const destructiveButton = "inline-flex min-h-9 items-center justify-center border border-[var(--error)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--error)] hover:bg-[var(--error-soft)] disabled:cursor-not-allowed disabled:opacity-50";

export function SecretsPage() {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<SecretKind>("ssh_password");
  const [value, setValue] = useState("");
  const [rotationId, setRotationId] = useState<string | null>(null);
  const [rotationValue, setRotationValue] = useState("");
  const secrets = useQuery({ queryKey: ["secrets"], queryFn: ({ signal }) => listSecrets(signal) });
  const audit = useQuery({ queryKey: ["secret-audit"], queryFn: ({ signal }) => listSecretAudit(signal) });
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["secrets"] });
    void queryClient.invalidateQueries({ queryKey: ["secret-audit"] });
  };
  const create = useMutation({
    mutationFn: () => createSecret({ name, kind, scope: { type: "global" }, value }),
    onSuccess: () => { setName(""); setValue(""); refresh(); },
  });
  const rotate = useMutation({
    mutationFn: () => rotateSecret(rotationId ?? "", rotationValue),
    onSuccess: () => { setRotationId(null); setRotationValue(""); refresh(); },
  });
  const revoke = useMutation({ mutationFn: (id: string) => revokeSecret(id), onSuccess: refresh });
  const remove = useMutation({ mutationFn: (id: string) => deleteSecret(id), onSuccess: refresh });

  function submit(event: Readonly<{ preventDefault: () => void }>) {
    event.preventDefault();
    if (name.trim() && value) create.mutate();
  }

  return (
    <>
      <header className="mb-4 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Sicherheit</p><h1>Secret Store</h1><p className="text-[var(--muted)]">Zugangsdaten zentral verwalten. Werte werden verschlüsselt gespeichert und nie wieder angezeigt.</p></div></header>
      <section className="mb-4 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
        <div className="mb-4 flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Neuer Eintrag</p><h2 className="mb-0">Secret hinzufügen</h2></div><span className="text-xs text-[var(--muted)]">Nach dem Speichern ist der Secret-Wert nicht abrufbar.</span></div>
        <form className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3" onSubmit={submit}>
          <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Interner Anzeigename. Der eigentliche Secret-Wert wird später nicht angezeigt."><span>Name</span><input className="w-full" value={name} onChange={(event) => setName(event.target.value)} autoComplete="off" required /></label>
          <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Der Typ legt fest, für welchen Verbindungs- oder Authentifizierungszweck das Secret verwendet werden darf."><span>Typ</span><select className="w-full" value={kind} onChange={(event) => setKind(event.target.value as SecretKind)}>{secretKinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label>
          <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Der Wert wird verschlüsselt gespeichert und nach dem Speichern nicht mehr ausgegeben."><span>Secret-Wert</span><input className="w-full" type="password" value={value} onChange={(event) => setValue(event.target.value)} autoComplete="new-password" required /></label>
          <button className={`${primaryButton} justify-self-start md:col-span-2 xl:col-span-3`} type="submit" disabled={create.isPending || name.trim() === "" || value === ""}>{create.isPending ? "Speichert…" : "Secret speichern"}</button>
        </form>
        {create.error ? <p className="mt-3 border border-[var(--error)] bg-[var(--error-soft)] p-3 text-sm font-semibold text-[var(--error)]" role="alert">{formatSecretError(create.error)}</p> : null}
      </section>
      <section className="mb-4 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
        <div className="mb-4 flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Zugangsdaten</p><h2 className="mb-0">Verwendbare Secrets</h2></div><span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2 py-1 text-xs text-[var(--muted)]">{secrets.data?.length ?? 0} Einträge · Werte nicht abrufbar</span></div>
        {queryContent(secrets.isLoading, Boolean(secrets.error), "Lade Secrets…", `Secrets konnten nicht geladen werden: ${formatSecretError(secrets.error)}`, <ul className="m-0 grid list-none gap-3 p-0">
          {(secrets.data ?? []).map((item) => {
            const metadata = item.metadata.metadata;
            const revoked = item.metadata.status === "revoked";
            return <li key={metadata.id}>
              <article className="grid gap-4 border border-[var(--line)] bg-[var(--paper-muted)] p-4 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2"><h3 className="m-0 break-all text-base font-semibold">{metadata.name}</h3><span className="border border-[var(--line)] bg-[var(--panel)] px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">{secretKindLabel(metadata.kind)}</span><span className={`inline-flex items-center px-2 py-1 text-xs font-bold ${revoked ? "bg-[var(--error-soft)] text-[var(--error)]" : "bg-[var(--success-soft)] text-[var(--success)]"}`}>{revoked ? "Widerrufen" : "Aktiv"}</span></div>
                  <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-[var(--muted)]"><span>Umfang: {secretScopeLabel(metadata.scope)}</span><span title={new Date(metadata.created_at).toLocaleString()}>Erstellt: {new Date(metadata.created_at).toLocaleDateString()}</span><span>Wert verborgen</span></div>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <button className={secondaryButton} type="button" onClick={() => { setRotationId(metadata.id); setRotationValue(""); }} disabled={revoked || rotate.isPending}>Rotieren</button>
                  <button className={secondaryButton} type="button" onClick={() => revoke.mutate(metadata.id)} disabled={revoked || revoke.isPending}>Widerrufen</button>
                  <button className={destructiveButton} type="button" onClick={() => remove.mutate(metadata.id)} disabled={remove.isPending}>Löschen</button>
                </div>
                {rotationId === metadata.id ? <form className="grid gap-3 border-t border-[var(--line)] pt-3 sm:grid-cols-[minmax(0,1fr)_auto_auto] sm:items-end lg:col-span-2" onSubmit={(event) => { event.preventDefault(); if (rotationValue) rotate.mutate(); }}><label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]"><span>Neuer Wert für „{metadata.name}“</span><input className="w-full" type="password" value={rotationValue} onChange={(event) => setRotationValue(event.target.value)} autoComplete="new-password" placeholder="Neuen Secret-Wert eingeben" required /></label><button className={primaryButton} type="submit" disabled={rotate.isPending || rotationValue === ""}>{rotate.isPending ? "Wird rotiert…" : "Rotation bestätigen"}</button><button className={secondaryButton} type="button" onClick={() => { setRotationId(null); setRotationValue(""); }}>Abbrechen</button></form> : null}
                {rotationId === metadata.id && rotate.error ? <p className="m-0 text-sm font-semibold text-[var(--error)]" role="alert">{formatSecretError(rotate.error)}</p> : null}
                {revoke.error ? <p className="m-0 text-sm font-semibold text-[var(--error)]" role="alert">{formatSecretError(revoke.error)}</p> : null}
                {remove.error ? <p className="m-0 text-sm font-semibold text-[var(--error)]" role="alert">{formatSecretError(remove.error)}</p> : null}
              </article>
            </li>;
          })}
          {secrets.data?.length === 0 ? <li className="grid min-h-28 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-4 text-center text-sm text-[var(--muted)]">Noch keine Secrets angelegt. Lege oben ein Secret an, um es einem Ziel zuzuweisen.</li> : null}
        </ul>)}
      </section>
      <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
        <div className="mb-4 flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Nachvollziehbarkeit</p><h2 className="mb-0">Secret-Audit</h2></div><span className="text-xs text-[var(--muted)]">Lebenszyklus-Ereignisse</span></div>
        {queryContent(audit.isLoading, Boolean(audit.error), "Lade Audit-Ereignisse…", `Audit konnte nicht geladen werden: ${formatSecretError(audit.error)}`, <ul className="m-0 grid list-none gap-2 p-0">
          {(audit.data ?? []).map((event, index) => <li className="grid gap-2 border border-[var(--line)] bg-[var(--paper-muted)] p-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center" key={`${event.secret_id}-${event.occurred_at}-${index}`}>
            <div className="min-w-0"><strong className="block">{secretAuditActionLabel(event.action)}</strong><span className="mt-1 block break-all text-xs text-[var(--muted)]">Secret-ID: {event.secret_id}</span><span className="mt-1 block text-xs text-[var(--muted)]">{new Date(event.occurred_at).toLocaleString()} · durch {event.role}</span></div>
            {event.related_job_id ? <Link className={secondaryButton} to={`/workflows/${event.related_job_id}`}>Workflow öffnen <span className="ml-2" aria-hidden="true">→</span></Link> : null}
          </li>)}
          {audit.data?.length === 0 ? <li className="grid min-h-20 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-4 text-center text-sm text-[var(--muted)]">Noch keine Secret-Lebenszyklus-Ereignisse vorhanden.</li> : null}
        </ul>)}
      </section>
    </>
  );
}

function queryContent(isLoading: boolean, hasError: boolean, loadingMessage: string, errorMessage: string, content: React.ReactNode) {
  if (isLoading) return <output className="block text-[var(--muted)]">{loadingMessage}</output>;
  if (hasError) return <p className="border border-[var(--error)] bg-[var(--error-soft)] p-3 text-sm font-semibold text-[var(--error)]" role="alert">{errorMessage}</p>;
  return content;
}

function secretKindLabel(kind: SecretKind) {
  const label = secretKinds.find((item) => item.value === kind)?.label;
  if (kind === "ssh_password") return "SSH-Passwort";
  if (kind === "ssh_private_key") return "SSH-Schlüssel";
  return label ?? "Allgemein";
}

function secretScopeLabel(scope: { type: "global" } | { type: "node"; id: string } | { type: "container"; id: number }) {
  if (scope.type === "node") return `Node ${scope.id}`;
  if (scope.type === "container") return `LXC ${scope.id}`;
  return "Global";
}

function secretAuditActionLabel(action: string) {
  const labels: Record<string, string> = {
    created: "Secret erstellt",
    rotated: "Secret rotiert",
    revoked: "Secret widerrufen",
    deleted: "Secret gelöscht",
    agent_reconfiguration_queued: "Agent-Neukonfiguration gestartet",
  };
  return labels[action] ?? action.replaceAll("_", " ");
}

function formatSecretError(error: unknown): string {
  if (error instanceof ApiError) {
    const details = [error.code, error.requestId ? `Request-ID ${error.requestId}` : undefined].filter(Boolean).join(" · ");
    return details ? `${error.message} (${details})` : error.message;
  }
  return error instanceof Error ? error.message : "Das Secret konnte nicht gespeichert werden.";
}
