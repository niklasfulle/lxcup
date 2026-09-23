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
  { value: "generic", label: "Allgemein" },
];

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
      <header className="page-header"><div><p className="eyebrow">Sicherheit</p><h1>Secret Store</h1><p className="muted">Nur Metadaten werden angezeigt. Secret-Werte verlassen diese Seite nach dem Schreibvorgang.</p></div></header>
      <section className="panel">
        <h2>Secret hinzufügen</h2>
        <form className="workflow-grid" onSubmit={submit}>
          <label title="Interner Anzeigename. Der eigentliche Secret-Wert wird später nicht angezeigt.">Name<input value={name} onChange={(event) => setName(event.target.value)} autoComplete="off" required /></label>
          <label title="Der Typ legt fest, für welchen Verbindungs- oder Authentifizierungszweck das Secret verwendet werden darf.">Typ<select value={kind} onChange={(event) => setKind(event.target.value as SecretKind)}>{secretKinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label>
          <label title="Der Wert wird verschlüsselt gespeichert und nach dem Speichern nicht mehr ausgegeben.">Wert<input type="password" value={value} onChange={(event) => setValue(event.target.value)} autoComplete="new-password" required /></label>
          <button className="primary-button" type="submit" disabled={create.isPending}>{create.isPending ? "Speichert…" : "Secret speichern"}</button>
        </form>
        {create.error ? <p className="error-state" role="alert">{formatSecretError(create.error)}</p> : null}
      </section>
      <section className="panel">
        <div className="section-heading"><h2>Verwendbare Secrets</h2><span className="muted">Werte sind nicht abrufbar</span></div>
        {secrets.isLoading ? <p className="muted">Lade Secrets…</p> : <div className="secret-list">
          {(secrets.data ?? []).map((item) => {
            const metadata = item.metadata.metadata;
            const revoked = item.metadata.status === "revoked";
            return <article className="secret-row" key={metadata.id}>
              <div><strong>{metadata.name}</strong><span className="muted">{metadata.kind} · {item.metadata.status}</span></div>
              <div className="secret-actions">
                <button type="button" onClick={() => { setRotationId(metadata.id); setRotationValue(""); }} disabled={revoked}>Rotieren</button>
                <button type="button" onClick={() => revoke.mutate(metadata.id)} disabled={revoked || revoke.isPending}>Widerrufen</button>
                <button type="button" onClick={() => remove.mutate(metadata.id)} disabled={remove.isPending}>Löschen</button>
              </div>
              {rotationId === metadata.id ? <form className="inline-form" onSubmit={(event) => { event.preventDefault(); if (rotationValue) rotate.mutate(); }}><input type="password" value={rotationValue} onChange={(event) => setRotationValue(event.target.value)} autoComplete="new-password" placeholder="Neuer Wert" required /><button type="submit" disabled={rotate.isPending}>Bestätigen</button></form> : null}
            </article>;
          })}
          {secrets.data?.length === 0 ? <p className="muted">Noch keine Secrets angelegt.</p> : null}
        </div>}
      </section>
      <section className="panel">
        <div className="section-heading"><h2>Audit</h2><span className="muted">Lebenszyklus-Ereignisse</span></div>
        <ul className="audit-list">{(audit.data ?? []).slice().reverse().map((event, index) => <li key={`${event.secret_id}-${event.occurred_at}-${index}`}><strong>{event.action}</strong><span className="muted">{event.secret_id} · {new Date(event.occurred_at).toLocaleString()}</span></li>)}</ul>
      </section>
    </>
  );
}

function formatSecretError(error: unknown): string {
  if (error instanceof ApiError) {
    const details = [error.code, error.requestId ? `Request-ID ${error.requestId}` : undefined].filter(Boolean).join(" · ");
    return details ? `${error.message} (${details})` : error.message;
  }
  return error instanceof Error ? error.message : "Das Secret konnte nicht gespeichert werden.";
}
