import { cn, ui } from "../ui";
import { useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createAnsibleJob, createSecret, createTarget, listSecrets, type SecretKind, type SecretMetadata, type TargetKind, type TargetTransport } from "../api";
import { queryKeys, useAnsibleJob, useAnsibleJobs, useTargets } from "../queries";
import { TargetLifecycle } from "../components/TargetLifecycle";

const kinds: Array<{ value: TargetKind; label: string; transport: TargetTransport }> = [
  { value: "lxc", label: "LXC", transport: "ssh" },
  { value: "linux_server", label: "Linux-Server", transport: "ssh" },
  { value: "windows_server", label: "Windows-System", transport: "winrm" },
];

type TargetArea = TargetKind | undefined;

const areaContent: Record<Exclude<TargetArea, undefined>, { eyebrow: string; title: string; description: string; registrationTitle: string }> = {
  lxc: { eyebrow: "LXC-Container", title: "LXC-Container", description: "LXC-Container werden hier manuell als eigenständige Ressourcen angelegt und verwaltet.", registrationTitle: "LXC-Container hinzufügen" },
  linux_server: { eyebrow: "Server", title: "Linux-Server", description: "Linux-Server werden ausschließlich hier als eigenständige Ressourcen aufgenommen und verwaltet.", registrationTitle: "Serverzugang konfigurieren" },
  windows_server: { eyebrow: "Windows", title: "Windows-Server", description: "Windows-Systeme werden ausschließlich hier über einen WinRM-Zugang aufgenommen und verwaltet.", registrationTitle: "Windows-Zugang konfigurieren" },
};

export function buildBootstrapCommand(baseUrl?: string) {
  const configuredBase = baseUrl?.trim() || import.meta.env.VITE_BOOTSTRAP_BASE_URL?.trim() || globalThis.location?.origin || "http://localhost:5173";
  const scriptUrl = new URL("/bootstrap-lxcup-user.sh", configuredBase).toString();
  const shellUrl = `'${scriptUrl}'`;
  return [
    "set -Eeuo pipefail",
    'SUDO=""; if [ "$(id -u)" -ne 0 ]; then SUDO="sudo"; fi',
    'if ! command -v curl >/dev/null 2>&1; then',
    '  if command -v apt-get >/dev/null 2>&1; then DEBIAN_FRONTEND=noninteractive $SUDO apt-get update && DEBIAN_FRONTEND=noninteractive $SUDO apt-get install --yes curl',
    '  elif command -v dnf >/dev/null 2>&1; then $SUDO dnf install --assumeyes curl',
    '  elif command -v yum >/dev/null 2>&1; then $SUDO yum install --assumeyes curl',
    '  else echo "curl konnte nicht automatisch installiert werden." >&2; exit 1; fi',
    "fi",
    `curl -fsSL ${shellUrl} | $SUDO bash -s -- lxcup`,
  ].join("\n");
}

export function TargetsPage({ area }: Readonly<{ area?: TargetArea }>) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const secrets = useQuery({ queryKey: ["secrets"], queryFn: ({ signal }) => listSecrets(signal) });
  const [name, setName] = useState("");
  const [address, setAddress] = useState("");
  const [sshUser, setSshUser] = useState("lxcup");
  const [kind, setKind] = useState<TargetKind>(area ?? "lxc");
  const [credentialSecret, setCredentialSecret] = useState("");
  const [agentSecret, setAgentSecret] = useState("");
  const [knownHostsSecret, setKnownHostsSecret] = useState("");
  const [newSecretFor, setNewSecretFor] = useState<"credential" | "known_hosts" | "agent" | null>(null);
  const [newSecretName, setNewSecretName] = useState("");
  const [newSecretKind, setNewSecretKind] = useState<SecretKind>("ssh_password");
  const [newSecretValue, setNewSecretValue] = useState("");
  const [createdTargetId, setCreatedTargetId] = useState<string>();
  const [startOnboarding, setStartOnboarding] = useState(true);
  const [deploymentJobId, setDeploymentJobId] = useState<string>();
  const [healthJobId, setHealthJobId] = useState<string>();
  const [bootstrapCopied, setBootstrapCopied] = useState(false);
  const healthStartedFor = useRef<string | undefined>(undefined);
  const inventoryStartedFor = useRef<string | undefined>(undefined);
  const content = area ? areaContent[area] : { eyebrow: "Automatisierung", title: "Zugänge & Agenten", description: "Zugangsprofile verbinden Infrastrukturressourcen mit kontrollierten Automatisierungs-Workflows.", registrationTitle: "Zugangsprofil anlegen" };
  const availableKinds = area ? kinds.filter((item) => item.value === area) : kinds;
  const selectedKind = kinds.find((item) => item.value === kind)!;
  const activeSecrets = (secrets.data ?? []).filter((item) => item.metadata.status === "active");

  const inlineSecret = useMutation({
    mutationFn: () => createSecret({ name: newSecretName.trim(), kind: newSecretKind, scope: { type: "global" }, value: newSecretValue }),
    onSuccess: (secret) => {
      if (newSecretFor === "credential") setCredentialSecret(secret.metadata.metadata.id);
      if (newSecretFor === "known_hosts") setKnownHostsSecret(secret.metadata.metadata.id);
      if (newSecretFor === "agent") setAgentSecret(secret.metadata.metadata.id);
      setNewSecretFor(null);
      setNewSecretName("");
      setNewSecretValue("");
      void queryClient.invalidateQueries({ queryKey: ["secrets"] });
    },
  });

  const deployment = useMutation({
    mutationFn: (targetId: string) => createAnsibleJob({ operation: "deploy_agent", target_id: targetId, mode: "apply", parameters: { operation: "deploy_agent", agent_version: "0.2.0" }, idempotency_key: `onboarding-deploy-${targetId}`, confirmed: true }),
    onSuccess: (job) => { setDeploymentJobId(job.id); void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }); },
  });
  const deploymentJob = useAnsibleJob(deploymentJobId);
  const health = useMutation({
    mutationFn: (targetId: string) => createAnsibleJob({ operation: "health_check", target_id: targetId, mode: "check", parameters: { operation: "health_check" }, idempotency_key: `onboarding-health-${targetId}`, confirmed: true }),
    onSuccess: (job) => { setHealthJobId(job.id); void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }); },
  });
  const healthJob = useAnsibleJob(healthJobId);
  const inventory = useMutation({
    mutationFn: (targetId: string) => createAnsibleJob({ operation: "collect_package_inventory", target_id: targetId, mode: "check", parameters: { operation: "collect_package_inventory" }, idempotency_key: `onboarding-inventory-${targetId}`, confirmed: true }),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }); },
  });
  const create = useMutation({
    mutationFn: () => createTarget({
      name,
      address,
      kind,
      transport: selectedKind.transport,
      ssh_user: selectedKind.transport === "ssh" ? sshUser.trim() || null : null,
      credential_secret_ref: credentialSecret,
      ssh_known_hosts_secret_ref: selectedKind.transport === "ssh" ? knownHostsSecret || null : null,
      agent_secret_ref: agentSecret,
    }),
    onSuccess: (target) => {
      setCreatedTargetId(target.id);
      setDeploymentJobId(undefined);
      setHealthJobId(undefined);
      setName("");
      setAddress("");
      void queryClient.invalidateQueries({ queryKey: queryKeys.targets });
      if (startOnboarding) deployment.mutate(target.id);
    },
  });

  const createdTarget = targets.data?.find((target) => target.id === createdTargetId) ?? create.data;
  const visibleTargets = area ? (targets.data ?? []).filter((target) => target.kind === area) : (targets.data ?? []);
  const pendingTargets = visibleTargets.filter((target) => target.state === "pending" && target.id !== createdTargetId);

  async function copyBootstrapCommand() {
    try {
      await copyText(buildBootstrapCommand());
      setBootstrapCopied(true);
      window.setTimeout(() => setBootstrapCopied(false), 2500);
    } catch {
      setBootstrapCopied(false);
    }
  }

  useEffect(() => {
    if (startOnboarding === false || createdTarget === undefined || deploymentJob.data?.status !== "succeeded" || createdTarget.state !== "managed" || healthStartedFor.current === createdTarget.id) return;
    healthStartedFor.current = createdTarget.id;
    health.mutate(createdTarget.id);
  }, [createdTarget, deploymentJob.data, health, startOnboarding]);
  useEffect(() => {
    if (startOnboarding === false || createdTarget === undefined || healthJob.data?.status !== "succeeded" || inventoryStartedFor.current === createdTarget.id) return;
    inventoryStartedFor.current = createdTarget.id;
    inventory.mutate(createdTarget.id);
  }, [createdTarget, healthJob.data, inventory, startOnboarding]);

  return (
    <>
      <header className={ui.pageHeader}>
        <div>
          <p className={ui.eyebrow}>{content.eyebrow}</p>
          <h1>{content.title}</h1>
          <p className={ui.muted}>{content.description}</p>
        </div>
      </header>

      {area === undefined ? <ResourceRelationshipMap /> : null}

      <section className={ui.panel}>
        <div className={ui.sectionHeading}>
          <div>
            <h2>{content.registrationTitle}</h2>
            <p className={ui.muted}>Verbindungsdaten und Secret-Referenzen bleiben auf diese Ressourcenart begrenzt.</p>
          </div>
          <span className={ui.stepBadge}>Schritt 1 · Zugang</span>
        </div>
        <div className={ui.registrationLayout}>
          <div className={ui.registrationForm}>
            <div className={ui.formSectionHeading}>
              <div>
                <h3>Verbindungsdaten</h3>
                <p className={ui.muted}>Diese Angaben verwendet der Worker für SSH und das Agent-Onboarding.</p>
              </div>
            </div>
            <TargetForm availableKinds={availableKinds} selectedKind={selectedKind} activeSecrets={activeSecrets} name={name} address={address} sshUser={sshUser} kind={kind} credentialSecret={credentialSecret} agentSecret={agentSecret} knownHostsSecret={knownHostsSecret} newSecretFor={newSecretFor} newSecretName={newSecretName} newSecretKind={newSecretKind} newSecretValue={newSecretValue} startOnboarding={startOnboarding} onboardingAvailable={area !== "lxc"} submitLabel={area === "lxc" ? "Zugang speichern & LXC wählen" : "Ziel registrieren"} inlineSecretPending={inlineSecret.isPending} inlineSecretError={inlineSecret.error instanceof Error ? inlineSecret.error.message : undefined} createPending={create.isPending} createError={create.error instanceof Error ? create.error.message : undefined} onSubmit={(event) => { event.preventDefault(); create.mutate(); }} onNameChange={setName} onAddressChange={setAddress} onSshUserChange={setSshUser} onKindChange={setKind} onCredentialChange={setCredentialSecret} onAgentChange={setAgentSecret} onKnownHostsChange={setKnownHostsSecret} onSecretForChange={setNewSecretFor} onSecretNameChange={setNewSecretName} onSecretKindChange={setNewSecretKind} onSecretValueChange={setNewSecretValue} onStartOnboardingChange={setStartOnboarding} onCreateSecret={() => inlineSecret.mutate()} />
          </div>
          {selectedKind.transport === "ssh" ? <BootstrapCard copied={bootstrapCopied} onCopy={() => void copyBootstrapCommand()} /> : <WindowsSetupCard />}
        </div>
      </section>

      {createdTarget ? <TargetLifecycle target={createdTarget} /> : null}
      {createdTarget && startOnboarding ? <OnboardingActivities deployment={deployment} health={health} inventory={inventory} /> : null}

      {pendingTargets.length > 0 && <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Offene Onboardings</h2><p className={ui.muted}>Diese Ziele warten noch auf Agent und Heartbeat.</p></div><span className={cn(ui.statusBadge, ui.statusPending)}>{pendingTargets.length} offen</span></div><div className={ui.lifecycleList}>{pendingTargets.map((target) => <TargetLifecycle key={target.id} target={target} />)}</div></section>}

      <section className={ui.panel}>
        <div className={ui.sectionHeading}>
          <div>
            <h2>{area ? `${content.title}-Inventar` : "Zugangsprofil-Inventar"}</h2>
            <p className={ui.muted}>Nur Zugänge dieser Ressourcenart</p>
          </div>
          <span className={ui.muted}>{visibleTargets.length} Einträge</span>
        </div>
        <TargetInventory targets={visibleTargets} isLoading={targets.isLoading} jobs={jobs.data ?? []} />
      </section>
    </>
  );
}

async function copyText(value: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) throw new Error("Die Zwischenablage ist in diesem Browser nicht verfügbar.");
}

function generateSecretValue() {
  const bytes = new Uint8Array(24);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function BootstrapCard({ copied, onCopy }: Readonly<{ copied: boolean; onCopy: () => void }>) {
  return <aside className={ui.bootstrapCard} aria-labelledby="bootstrap-card-title">
    <p className={ui.eyebrow}>Host vorbereiten</p>
    <h3 id="bootstrap-card-title">lxcup-Benutzer anlegen</h3>
    <p className={ui.muted}>Führe den vorbereiteten Befehl einmal als root auf dem Zielhost aus. Er installiert bei Bedarf curl und legt danach den eingeschränkten SSH-Benutzer an.</p>
    <ol className={ui.bootstrapSteps}>
      <li><span>1</span><span>Auf dem Zielhost anmelden</span></li>
      <li><span>2</span><span>Befehl kopieren und ausführen</span></li>
      <li><span>3</span><span>Passwort im Deployment-Secret hinterlegen</span></li>
    </ol>
    <button className={cn(ui.button, "self-start")} type="button" onClick={onCopy} title="Kopiert den Bootstrap-Befehl für den lxcup-Benutzer auf dem Zielhost.">
      {copied ? "✓ Befehl kopiert" : "＋ Installationsbefehl kopieren"}
    </button>
    <span className={ui.copyStatus} aria-live="polite">{copied ? "Der Befehl liegt jetzt in der Zwischenablage." : ""}</span>
    <p className={ui.bootstrapNote}>Die Frontend-URL muss vom Zielhost erreichbar sein.</p>
  </aside>;
}

function WindowsSetupCard() {
  return <aside className={ui.bootstrapCard} aria-labelledby="windows-setup-title">
    <p className={ui.eyebrow}>Windows vorbereiten</p>
    <h3 id="windows-setup-title">WinRM-Zugang prüfen</h3>
    <p className={ui.muted}>Stelle vor dem Speichern sicher, dass der Windows-Host über WinRM erreichbar ist und das ausgewählte Secret die hinterlegten Zugangsdaten enthält.</p>
    <ol className={ui.bootstrapSteps}>
      <li><span>1</span><span>WinRM auf dem Server aktivieren</span></li>
      <li><span>2</span><span>Zugang als Deployment-Secret hinterlegen</span></li>
      <li><span>3</span><span>Windows-Server registrieren</span></li>
    </ol>
  </aside>;
}

function ResourceRelationshipMap() {
  return <section className={ui.relationshipMap} aria-label="Zusammenspiel von Inventar und Automatisierung">
    <article><span className={ui.relationshipStep}>1</span><div><strong>Infrastrukturinventar</strong><p>Nodes, LXC- &amp; Docker-Container werden entdeckt und bleiben in ihren eigenen Bereichen.</p><Link to="/containers">LXC-Inventar öffnen →</Link></div></article>
    <span className={ui.relationshipArrow} aria-hidden="true">→</span>
    <article><span className={ui.relationshipStep}>2</span><div><strong>Zugangsprofil</strong><p>Adresse, Secret-Referenzen und Agent-Token beschreiben die Verbindung – nicht den Container selbst.</p></div></article>
    <span className={ui.relationshipArrow} aria-hidden="true">→</span>
    <article><span className={ui.relationshipStep}>3</span><div><strong>LXC-Onboarding</strong><p>Verknüpft einen entdeckten LXC mit seinem Zugangsprofil und startet den Agenten.</p><Link to="/enrollments/new">LXC aufnehmen →</Link></div></article>
  </section>;
}

type TargetFormProps = Readonly<{
  availableKinds: Array<{ value: TargetKind; label: string; transport: TargetTransport }>;
  selectedKind: (typeof kinds)[number];
  activeSecrets: SecretMetadata[];
  name: string;
  address: string;
  sshUser: string;
  kind: TargetKind;
  credentialSecret: string;
  agentSecret: string;
  knownHostsSecret: string;
  newSecretFor: "credential" | "known_hosts" | "agent" | null;
  newSecretName: string;
  newSecretKind: SecretKind;
  newSecretValue: string;
  startOnboarding: boolean;
  onboardingAvailable: boolean;
  submitLabel: string;
  inlineSecretPending: boolean;
  inlineSecretError?: string;
  createPending: boolean;
  createError?: string;
  onSubmit: (event: { preventDefault: () => void }) => void;
  onNameChange: (value: string) => void;
  onAddressChange: (value: string) => void;
  onSshUserChange: (value: string) => void;
  onKindChange: (value: TargetKind) => void;
  onCredentialChange: (value: string) => void;
  onAgentChange: (value: string) => void;
  onKnownHostsChange: (value: string) => void;
  onSecretForChange: (value: "credential" | "known_hosts" | "agent" | null) => void;
  onSecretNameChange: (value: string) => void;
  onSecretKindChange: (value: SecretKind) => void;
  onSecretValueChange: (value: string) => void;
  onStartOnboardingChange: (value: boolean) => void;
  onCreateSecret: () => void;
}>;

function TargetForm(props: TargetFormProps) {
  const { selectedKind, activeSecrets, newSecretFor, newSecretName, newSecretKind, newSecretValue, inlineSecretPending, createPending, createError, inlineSecretError } = props;
  return <form onSubmit={props.onSubmit}>
    <div className={ui.workflowGrid}>
      <label><span className={ui.fieldLabel} title="Anzeigename des verwalteten Ziels.">Name</span><input name="target_name" autoComplete="off" value={props.name} onChange={(event) => props.onNameChange(event.target.value)} required /></label>
      <label><span className={ui.fieldLabel} title="Plattform des Ziels. Sie bestimmt unter anderem das verwendete Ansible-Playbook.">Typ</span>{props.availableKinds.length === 1 ? <input name="target_kind" value={selectedKind.label} readOnly /> : <select name="target_kind" value={props.kind} onChange={(event) => props.onKindChange(event.target.value as TargetKind)}>{props.availableKinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select>}</label>
      <label><span className={ui.fieldLabel} title="IP-Adresse oder DNS-Name, unter dem der Worker das Ziel erreicht.">Adresse</span><input name="target_address" autoComplete="url" value={props.address} onChange={(event) => props.onAddressChange(event.target.value)} placeholder="IP oder DNS-Name …" required /></label>
      {selectedKind.transport === "ssh" && <label><span className={ui.fieldLabel} title="Benutzername für die SSH-Verbindung zu diesem Ziel.">SSH-Benutzer</span><input name="ssh_user" autoComplete="username" value={props.sshUser} onChange={(event) => props.onSshUserChange(event.target.value)} placeholder="z. B. root oder lxcup …" required /></label>}
      <SecretSelect label="Deployment-Secret" title="Zugangsdaten für die Verbindung zum Ziel, zum Beispiel ein SSH-Passwort oder ein SSH-Private-Key." value={props.credentialSecret} options={activeSecrets} onChange={props.onCredentialChange} onNew={() => { props.onSecretForChange("credential"); props.onSecretKindChange(selectedKind.transport === "ssh" ? "ssh_password" : "generic"); }} />
      {selectedKind.transport === "ssh" && <SecretSelect label="SSH-Host-Fingerprint" title="Bekannter SSH-Host-Fingerprint als known_hosts-Datei." value={props.knownHostsSecret} options={activeSecrets.filter((item) => item.metadata.metadata.kind === "ssh_known_hosts")} onChange={props.onKnownHostsChange} onNew={() => { props.onSecretForChange("known_hosts"); props.onSecretKindChange("ssh_known_hosts"); }} emptyLabel="Known-Hosts-Secret auswählen" />}
      <SecretSelect label="Agent-Token" title="Geheimer Token, mit dem sich der installierte lxcup-Agent beim Controller authentifiziert." value={props.agentSecret} options={activeSecrets} onChange={props.onAgentChange} onNew={() => { props.onSecretForChange("agent"); props.onSecretKindChange("agent_token"); }} />
      <label><span className={ui.fieldLabel} title="Verbindungsprotokoll, das automatisch aus dem Zieltyp abgeleitet wird.">Transport</span><input name="transport" value={selectedKind.transport.toUpperCase()} readOnly /></label>
    </div>
    {newSecretFor && <InlineSecretEditor newSecretFor={newSecretFor} name={newSecretName} kind={newSecretKind} value={newSecretValue} pending={inlineSecretPending} error={inlineSecretError} onCancel={() => props.onSecretForChange(null)} onNameChange={props.onSecretNameChange} onKindChange={props.onSecretKindChange} onValueChange={props.onSecretValueChange} onGenerate={() => props.onSecretValueChange(generateSecretValue())} onCreate={props.onCreateSecret} />}
    {props.onboardingAvailable ? <label className={ui.confirmField}><input type="checkbox" checked={props.startOnboarding} onChange={(event) => props.onStartOnboardingChange(event.target.checked)} /> Onboarding direkt starten: Agent installieren und nach erfolgreichem Heartbeat einen Healthcheck ausführen.</label> : null}
     <button className={cn(ui.primaryButton, "justify-self-center px-8 min-w-60")} type="submit" disabled={createPending || props.credentialSecret === "" || props.agentSecret === "" || (selectedKind.transport === "ssh" && props.knownHostsSecret === "")}>{createPending ? "Wird angelegt…" : props.submitLabel}</button>
    {createError && <p className={ui.errorState} role="alert">{createError}</p>}
  </form>;
}

function SecretSelect({ label, title, value, options, onChange, onNew, emptyLabel = "Secret auswählen" }: Readonly<{ label: string; title: string; value: string; options: SecretMetadata[]; onChange: (value: string) => void; onNew: () => void; emptyLabel?: string }>) {
  return <label><span className={ui.fieldLabel} title={title}>{label}</span><div className={ui.inlineField}><select value={value} onChange={(event) => onChange(event.target.value)} required><option value="">{emptyLabel}</option>{options.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select><button className={ui.button} type="button" onClick={onNew}>＋ Neu</button></div></label>;
}

function InlineSecretEditor({ newSecretFor, name, kind, value, pending, error, onCancel, onNameChange, onKindChange, onValueChange, onGenerate, onCreate }: Readonly<{ newSecretFor: "credential" | "known_hosts" | "agent"; name: string; kind: SecretKind; value: string; pending: boolean; error?: string; onCancel: () => void; onNameChange: (value: string) => void; onKindChange: (value: SecretKind) => void; onValueChange: (value: string) => void; onGenerate: () => void; onCreate: () => void }>) {
  return <fieldset className={ui.inlineSecretEditor}><legend>Neues {secretPurposeLabel(newSecretFor)}</legend><button className={ui.textLink} type="button" onClick={onCancel}>Abbrechen</button><div className={ui.workflowGrid}><label><span>Name</span><input value={name} onChange={(event) => onNameChange(event.target.value)} placeholder="z. B. lxcup-test-ssh" autoComplete="off" /></label><label><span>Typ</span><select value={kind} onChange={(event) => onKindChange(event.target.value as SecretKind)}><option value="ssh_password">SSH Passwort</option><option value="ssh_private_key">SSH Private Key</option><option value="ssh_known_hosts">SSH Known Hosts</option><option value="agent_token">Agent-Token</option><option value="generic">Allgemein</option></select></label><label><span>Wert</span><input type="password" value={value} onChange={(event) => onValueChange(event.target.value)} autoComplete="new-password" placeholder="Wert eingeben oder erzeugen" /></label><button className={ui.button} type="button" onClick={onGenerate}>Wert erzeugen</button></div><button className={ui.primaryButton} type="button" disabled={pending || name.trim() === "" || value === ""} onClick={onCreate}>{pending ? "Speichert…" : "Secret erstellen und auswählen"}</button>{error && <p className={ui.errorState} role="alert">{error}</p>}</fieldset>;
}

function secretPurposeLabel(value: "credential" | "known_hosts" | "agent") {
  if (value === "credential") return "Deployment-Secret";
  if (value === "known_hosts") return "Known-Hosts-Secret";
  return "Agent-Token";
}

type JobMutationView = Readonly<{ isPending: boolean; data?: { id: string }; error: unknown }>;

function OnboardingActivities({ deployment, health, inventory }: Readonly<{ deployment: JobMutationView; health: JobMutationView; inventory: JobMutationView }>) {
  return <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Onboarding-Aktivitäten</h2><p className={ui.muted}>Jeder Schritt wird als eigener Workflow mit eigenem Protokoll geführt.</p></div></div><div className={ui.onboardingActivities}><div><strong>1. Agent installieren</strong>{jobActivity(deployment, "wird erstellt…")}{mutationError(deployment.error)}</div><div><strong>2. Healthcheck</strong>{jobActivity(health, "wird gestartet…", "Startet nach erfolgreicher Agent-Installation und Heartbeat.")}{mutationError(health.error)}</div><div><strong>3. Paketinventar</strong>{jobActivity(inventory, "wird erfasst…", "Startet nach erfolgreichem Healthcheck.")}{mutationError(inventory.error)}</div></div></section>;
}

function jobActivity(job: JobMutationView, pendingLabel: string, idleLabel?: string) {
  if (job.isPending) return <span className={ui.muted}> {pendingLabel}</span>;
  if (job.data) return <Link className={ui.textLink} to={`/workflows/${job.data.id}`}>Protokoll öffnen →</Link>;
  return idleLabel ? <p className={ui.muted}>{idleLabel}</p> : null;
}

function mutationError(error: unknown) {
  return error instanceof Error ? <p className={ui.errorState}>{error.message}</p> : null;
}

function TargetInventory({ targets, isLoading, jobs }: Readonly<{ targets: import("../api").TargetDto[]; isLoading: boolean; jobs: import("../api").AnsibleJobDto[] }>) {
  if (isLoading) return <p className={ui.muted}>Lade Zugangsprofile…</p>;
  if (targets.length === 0) return <p className={ui.emptyState}>Noch keine Zugangsprofile für diese Ressourcenart angelegt.</p>;
  return <div className={ui.tableWrap}><table><thead><tr><th>Name</th><th>Typ</th><th>Adresse</th><th>Transport</th><th>Agent</th><th>Status</th><th>Onboarding-Protokolle</th><th /></tr></thead><tbody>{targets.map((target) => <tr key={target.id}><td><Link className={ui.textLink} to={`/targets/${target.id}`}>{target.name}</Link></td><td>{target.kind}</td><td>{target.address}</td><td>{target.transport}</td><td title="Wird vom letzten authentifizierten Heartbeat des Zielsystems gemeldet.">{target.agent_version ? `v${target.agent_version}` : "Noch keine Meldung"}</td><td><span className={cn(ui.statusBadge, targetStateClass(target.state) === "success" ? ui.statusSuccess : targetStateClass(target.state) === "neutral" ? ui.statusNeutral : ui.statusPending)}>{targetStateLabel(target.state)}</span></td><td><TargetOnboardingProtocols target={target} jobs={jobs} /></td><td><Link className={ui.textLink} to={`/targets/${target.id}/packages`}>Inventar</Link></td></tr>)}</tbody></table></div>;
}

function TargetOnboardingProtocols({ target, jobs }: Readonly<{ target: import("../api").TargetDto; jobs: import("../api").AnsibleJobDto[] }>) {
  const targetJobs = jobs.filter((job) => "target" in job.target && job.target.target === target.id);
  const deployment = targetJobs.find((job) => job.operation === "deploy_agent");
  if (!deployment) return <span className={ui.muted}>Noch nicht gestartet</span>;
  const laterJobs = targetJobs.filter((job) => job.created_at >= deployment.created_at);
  const health = laterJobs.find((job) => job.operation === "health_check");
  const inventory = laterJobs.find((job) => job.operation === "collect_package_inventory");
  const steps = [
    ["Agent", deployment],
    ["Healthcheck", health],
    ["Paketinventar", inventory],
  ] as const;
  return <div className={ui.lifecycleList}>{steps.map(([label, job]) => job ? <Link className={ui.textLink} key={label} to={`/workflows/${job.id}`} title={`${label}: ${job.status}`}>{label} · {job.status}</Link> : <span className={ui.muted} key={label}>{label} · ausstehend</span>)}</div>;
}

function targetStateClass(state: "pending" | "managed" | "disabled") {
  if (state === "managed") return "success";
  if (state === "disabled") return "neutral";
  return "pending";
}

function targetStateLabel(state: "pending" | "managed" | "disabled") {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Pending";
}
