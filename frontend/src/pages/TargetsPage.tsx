import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createTarget, listSecrets, type TargetKind, type TargetTransport } from "../api";
import { queryKeys, useTargets } from "../queries";

const kinds: Array<{ value: TargetKind; label: string; transport: TargetTransport }> = [
  { value: "lxc", label: "LXC", transport: "ssh" },
  { value: "linux_server", label: "Linux-Server", transport: "ssh" },
  { value: "windows_server", label: "Windows-System", transport: "winrm" },
];

export function TargetsPage() {
  const queryClient = useQueryClient();
  const targets = useTargets();
  const secrets = useQuery({ queryKey: ["secrets"], queryFn: ({ signal }) => listSecrets(signal) });
  const [name, setName] = useState(""); const [address, setAddress] = useState(""); const [kind, setKind] = useState<TargetKind>("lxc"); const [credentialSecret, setCredentialSecret] = useState(""); const [agentSecret, setAgentSecret] = useState("");
  const selectedKind = kinds.find((item) => item.value === kind)!;
  const create = useMutation({ mutationFn: () => createTarget({ name, address, kind, transport: selectedKind.transport, credential_secret_ref: credentialSecret, agent_secret_ref: agentSecret }), onSuccess: () => { setName(""); setAddress(""); void queryClient.invalidateQueries({ queryKey: queryKeys.targets }); } });
  const activeSecrets = (secrets.data ?? []).filter((item) => item.metadata.status === "active");
  return <><header className="page-header"><div><p className="eyebrow">Inventar</p><h1>Ziele</h1><p className="muted">LXC, Linux und Windows werden mit demselben Ansible- und Agentenpfad verwaltet.</p></div></header><section className="panel workflow-panel"><h2>Ziel aufnehmen</h2><form onSubmit={(event) => { event.preventDefault(); create.mutate(); }}><div className="workflow-grid"><label>Name<input value={name} onChange={(event) => setName(event.target.value)} required /></label><label>Typ<select value={kind} onChange={(event) => setKind(event.target.value as TargetKind)}>{kinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label><label>Adresse<input value={address} onChange={(event) => setAddress(event.target.value)} placeholder="IP oder DNS-Name" required /></label><label>Deployment-Secret<select value={credentialSecret} onChange={(event) => setCredentialSecret(event.target.value)} required><option value="">Secret auswählen</option>{activeSecrets.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select></label><label>Agent-Token<select value={agentSecret} onChange={(event) => setAgentSecret(event.target.value)} required><option value="">Secret auswählen</option>{activeSecrets.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select></label><label>Transport<input value={selectedKind.transport.toUpperCase()} readOnly /></label></div><button className="primary-button" type="submit" disabled={create.isPending || !credentialSecret || !agentSecret}>{create.isPending ? "Wird angelegt…" : "Ziel anlegen"}</button>{create.error ? <p className="error-state">{create.error.message}</p> : null}</form></section><section className="panel"><h2>Target-Inventar</h2>{targets.isLoading ? <p className="muted">Lade Ziele…</p> : !targets.data?.length ? <p className="empty-state">Noch keine Ziele angelegt.</p> : <div className="table-wrap"><table><thead><tr><th>Name</th><th>Typ</th><th>Adresse</th><th>Transport</th><th>Status</th></tr></thead><tbody>{targets.data.map((target) => <tr key={target.id}><td>{target.name}</td><td>{target.kind}</td><td>{target.address}</td><td>{target.transport}</td><td>{target.state}</td></tr>)}</tbody></table></div>}</section></>;
}
