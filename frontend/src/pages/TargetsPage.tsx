import { cn } from "../classnames";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createAnsibleJob, createSecret, createTarget, listSecrets, type AnsibleJobDto, type SecretKind, type SecretMetadata, type TargetDto, type TargetKind, type TargetTransport } from "../api";
import { queryKeys, useAnsibleJob, useAnsibleJobs, usePackageInventory, useTargetTelemetry, useTargets } from "../queries";
import { TargetLifecycle } from "../components/TargetLifecycle";
import { isTelemetryStale } from "../telemetryFreshness";

const kinds: Array<{ value: TargetKind; label: string; transport: TargetTransport }> = [
  { value: "lxc", label: "LXC", transport: "ssh" },
  { value: "linux_server", label: "Linux-Server", transport: "ssh" },
  { value: "windows_server", label: "Windows-System", transport: "winrm" },
];

type TargetArea = TargetKind | undefined;
type TargetState = "pending" | "managed" | "disabled";

const areaContent: Record<Exclude<TargetArea, undefined>, { eyebrow: string; title: string; description: string; registrationTitle: string }> = {
  lxc: { eyebrow: "LXC-Container", title: "LXC-Container", description: "LXC-Container werden hier manuell als eigenständige Ressourcen angelegt und verwaltet.", registrationTitle: "LXC-Container hinzufügen" },
  linux_server: { eyebrow: "Server", title: "Linux-Server", description: "Linux-Server werden ausschließlich hier als eigenständige Ressourcen aufgenommen und verwaltet.", registrationTitle: "Serverzugang konfigurieren" },
  windows_server: { eyebrow: "Windows", title: "Windows-Server", description: "Windows-Systeme werden ausschließlich hier über einen WinRM-Zugang aufgenommen und verwaltet.", registrationTitle: "Windows-Zugang konfigurieren" },
};

const profileContent = { eyebrow: "Automatisierung", title: "Zugänge & Agenten", description: "Zugangsprofile verbinden Infrastrukturressourcen mit kontrollierten Automatisierungs-Workflows.", registrationTitle: "Zugangsprofil anlegen" };

function contentForArea(area: TargetArea) {
  return area === undefined ? profileContent : areaContent[area];
}

function kindsForArea(area: TargetArea) {
  return area === undefined ? kinds : kinds.filter((item) => item.value === area);
}

function targetsForArea(targets: import("../api").TargetDto[], area: TargetArea) {
  return area === undefined ? targets : targets.filter((target) => target.kind === area);
}

export function TargetsPage({ area }: Readonly<{ area?: TargetArea }>) {
  const queryClient = useQueryClient();
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
  const [addFormOpenByArea, setAddFormOpenByArea] = useState<Partial<Record<TargetKind | "profiles", boolean>>>({});
  const [startOnboarding, setStartOnboarding] = useState(true);
  const [deploymentJobId, setDeploymentJobId] = useState<string>();
  const [healthJobId, setHealthJobId] = useState<string>();
  const [bootstrapCopied, setBootstrapCopied] = useState(false);
  const healthStartedFor = useRef<string | undefined>(undefined);
  const inventoryStartedFor = useRef<string | undefined>(undefined);
  const content = contentForArea(area);
  const areaKey = area ?? "profiles";
  const addFormOpen = addFormOpenByArea[areaKey] ?? false;
  const availableKinds = kindsForArea(area);
  const selectedKind = kinds.find((item) => item.value === kind)!;
  const activeSecrets = (secrets.data ?? []).filter((item) => item.metadata.status === "active");

  const inlineSecret = useMutation({
    mutationFn: () => createSecret({ name: newSecretName.trim(), kind: newSecretKind, scope: { type: "global" }, value: newSecretValue }),
    onSuccess: (secret) => {
      const secretSetters = { credential: setCredentialSecret, known_hosts: setKnownHostsSecret, agent: setAgentSecret };
      if (newSecretFor !== null) secretSetters[newSecretFor](secret.metadata.metadata.id);
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
      setAddFormOpenByArea((current) => ({ ...current, [areaKey]: false }));
      void queryClient.invalidateQueries({ queryKey: queryKeys.targets });
      if (startOnboarding) deployment.mutate(target.id);
    },
  });

  const createdTarget = targets.data?.find((target) => target.id === createdTargetId) ?? create.data;
  const visibleTargets = targetsForArea(targets.data ?? [], area);
  const pendingTargets = visibleTargets.filter((target) => target.state === "pending" && target.id !== createdTargetId);

  async function copyBootstrapScript() {
    try {
      const response = await fetch("/bootstrap-lxcup-user.sh");
      if (!response.ok) throw new Error("Das Vorbereitungsskript konnte nicht geladen werden.");
      await copyText(await response.text());
      setBootstrapCopied(true);
      globalThis.setTimeout(() => setBootstrapCopied(false), 2500);
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
      <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start">
        <div>
          <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">{content.eyebrow}</p>
          <h1>{content.title}</h1>
          <p className="text-[var(--muted)]">{content.description}</p>
        </div>
        <button
          className={addFormOpen ? "inline-flex min-h-9 items-center justify-center gap-2 border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" : "inline-flex min-h-9 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50"}
          type="button"
          aria-expanded={addFormOpen}
          aria-controls="target-registration"
          onClick={() => setAddFormOpenByArea((current) => ({ ...current, [areaKey]: !current[areaKey] }))}
        >
          {addFormOpen ? "Schließen" : "Hinzufügen"}
        </button>
      </header>

      {area === undefined ? <ResourceRelationshipMap /> : null}

      {addFormOpen ? <section className="mb-3 overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)]" id="target-registration" aria-labelledby="target-registration-title">
        <header className="flex items-center justify-between gap-4 border-b border-[var(--line)] bg-[var(--paper-muted)] px-4 py-3 max-[720px]:items-start">
          <div className="min-w-0">
            <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Ressource einrichten</p>
            <h2 id="target-registration-title">{content.registrationTitle}</h2>
            <p className="text-sm text-[var(--muted)]">Zugang, Anmeldung und optionale Automatisierung konfigurieren.</p>
          </div>
          <span className="inline-flex shrink-0 items-center gap-2 border border-[var(--primary)] bg-[var(--primary-soft)] px-2.5 py-1.5 text-[10px] font-bold uppercase tracking-wider text-[var(--primary)]"><span className="grid h-5 w-5 place-items-center bg-lxcup-primary text-white">1</span>Zugang</span>
        </header>
        <div className="grid items-stretch xl:grid-cols-[minmax(0,1fr)_22rem]">
          <div className="min-w-0 p-4">
            <div className="mb-4 border-b border-[var(--line)] pb-3">
              <h3>Verbindungsdaten</h3>
              <p className="text-sm text-[var(--muted)]">Der Worker verwendet diese Angaben für die Verbindung zum Ziel.</p>
            </div>
            <TargetForm availableKinds={availableKinds} selectedKind={selectedKind} activeSecrets={activeSecrets} name={name} address={address} sshUser={sshUser} kind={kind} credentialSecret={credentialSecret} agentSecret={agentSecret} knownHostsSecret={knownHostsSecret} newSecretFor={newSecretFor} newSecretName={newSecretName} newSecretKind={newSecretKind} newSecretValue={newSecretValue} startOnboarding={startOnboarding} onboardingAvailable={area !== "lxc"} submitLabel="Hinzufügen" inlineSecretPending={inlineSecret.isPending} inlineSecretError={inlineSecret.error instanceof Error ? inlineSecret.error.message : undefined} createPending={create.isPending} createError={create.error instanceof Error ? create.error.message : undefined} onSubmit={(event) => { event.preventDefault(); create.mutate(); }} onNameChange={setName} onAddressChange={setAddress} onSshUserChange={setSshUser} onKindChange={setKind} onCredentialChange={setCredentialSecret} onAgentChange={setAgentSecret} onKnownHostsChange={setKnownHostsSecret} onSecretForChange={setNewSecretFor} onSecretNameChange={setNewSecretName} onSecretKindChange={setNewSecretKind} onSecretValueChange={setNewSecretValue} onStartOnboardingChange={setStartOnboarding} onCreateSecret={() => inlineSecret.mutate()} />
          </div>
          <div className="border-t border-[var(--line)] bg-[var(--paper-muted)] p-4 xl:border-l xl:border-t-0">
            {selectedKind.transport === "ssh" ? <BootstrapCard copied={bootstrapCopied} onCopy={() => void copyBootstrapScript()} /> : <WindowsSetupCard />}
          </div>
        </div>
      </section> : null}

      {createdTarget ? <TargetLifecycle target={createdTarget} /> : null}
      {createdTarget && startOnboarding ? <OnboardingActivities deployment={deployment} health={health} inventory={inventory} /> : null}

      {pendingTargets.length > 0 && <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Offene Onboardings</h2><p className="text-[var(--muted)]">Diese Ziele warten noch auf Agent und Heartbeat.</p></div><span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", "bg-[var(--warning-soft)] text-[var(--warning)]")}>{pendingTargets.length} offen</span></div><div className="grid">{pendingTargets.map((target) => <TargetLifecycle key={target.id} target={target} />)}</div></section>}

      <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">
        <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start">
          <div>
            <h2>{area ? `${content.title}-Inventar` : "Zugangsprofil-Inventar"}</h2>
            <p className="text-[var(--muted)]">Registrierte Ressourcen, Agent-Version und Onboarding-Status auf einen Blick.</p>
          </div>
          <span className="text-[var(--muted)]">{visibleTargets.length} Einträge</span>
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
  throw new Error("Die Zwischenablage ist in diesem Browser nicht verfügbar.");
}

function generateSecretValue() {
  const bytes = new Uint8Array(24);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function BootstrapCard({ copied, onCopy }: Readonly<{ copied: boolean; onCopy: () => void }>) {
  return <aside className="grid content-start gap-3" aria-labelledby="bootstrap-card-title">
    <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Host vorbereiten</p>
    <h3 id="bootstrap-card-title">lxcup-Benutzer anlegen</h3>
    <p className="text-[var(--muted)]">Kopiere das vollständige Bash-Skript, speichere es auf dem Zielhost und führe es dort als root aus. Es installiert bei Bedarf curl und sudo und legt danach den eingeschränkten SSH-Benutzer an.</p>
    <ol className="m-0 grid list-none gap-3 border-y border-[var(--line)] py-4 pl-0">
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">1</span><span>Auf dem Zielhost anmelden</span></li>
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">2</span><span>Skript als Datei speichern und ausführen</span></li>
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">3</span><span>Passwort im Deployment-Secret hinterlegen</span></li>
    </ol>
    <button className="inline-flex min-h-10 w-full items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onCopy} title="Kopiert das vollständige Bash-Vorbereitungsskript für den lxcup-Benutzer.">
      {copied ? "✓ Skript kopiert" : "Vorbereitungsskript kopieren"}
    </button>
    <span className="min-h-4 text-xs font-medium text-[var(--success)]" aria-live="polite">{copied ? "Das vollständige Skript liegt jetzt in der Zwischenablage." : ""}</span>
  </aside>;
}

function WindowsSetupCard() {
  return <aside className="grid content-start gap-3" aria-labelledby="windows-setup-title">
    <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Windows vorbereiten</p>
    <h3 id="windows-setup-title">WinRM-Zugang prüfen</h3>
    <p className="text-[var(--muted)]">Stelle vor dem Speichern sicher, dass der Windows-Host über WinRM erreichbar ist und das ausgewählte Secret die hinterlegten Zugangsdaten enthält.</p>
    <ol className="m-0 grid list-none gap-3 border-y border-[var(--line)] py-4 pl-0">
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">1</span><span>WinRM auf dem Server aktivieren</span></li>
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">2</span><span>Zugang als Deployment-Secret hinterlegen</span></li>
      <li className="flex items-center gap-2"><span className="grid h-5 w-5 shrink-0 place-items-center bg-[var(--panel)] text-[10px] font-bold text-lxcup-primary">3</span><span>Windows-Server registrieren</span></li>
    </ol>
  </aside>;
}

function ResourceRelationshipMap() {
  return <section className="mb-3 grid items-stretch gap-3 lg:grid-cols-[1fr_auto_1fr_auto_1fr] max-[720px]:grid-cols-1" aria-label="Zusammenspiel von Inventar und Automatisierung">
    <article><span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-lxcup-primary text-xs font-bold text-white">1</span><div><strong>Infrastrukturinventar</strong><p>Nodes, LXC- &amp; Docker-Container werden entdeckt und bleiben in ihren eigenen Bereichen.</p><Link to="/containers">LXC-Inventar öffnen →</Link></div></article>
    <span className="hidden self-center text-lg font-bold text-[var(--primary)] lg:block" aria-hidden="true">→</span>
    <article><span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-lxcup-primary text-xs font-bold text-white">2</span><div><strong>Zugangsprofil</strong><p>Adresse, Secret-Referenzen und Agent-Token beschreiben die Verbindung – nicht den Container selbst.</p></div></article>
    <span className="hidden self-center text-lg font-bold text-[var(--primary)] lg:block" aria-hidden="true">→</span>
    <article><span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-lxcup-primary text-xs font-bold text-white">3</span><div><strong>LXC-Onboarding</strong><p>Verknüpft einen entdeckten LXC mit seinem Zugangsprofil und startet den Agenten.</p><Link to="/enrollments/new">LXC aufnehmen →</Link></div></article>
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
  return <form className="grid gap-4" onSubmit={props.onSubmit}>
    <div className="grid grid-cols-1 gap-x-4 gap-y-3 sm:grid-cols-2">
      <label><span className="inline-flex items-center gap-1" title="Anzeigename des verwalteten Ziels.">Name</span><input name="target_name" autoComplete="off" value={props.name} onChange={(event) => props.onNameChange(event.target.value)} required /></label>
      <label><span className="inline-flex items-center gap-1" title="Plattform des Ziels. Sie bestimmt unter anderem das verwendete Ansible-Playbook.">Typ</span>{props.availableKinds.length === 1 ? <input name="target_kind" value={selectedKind.label} readOnly /> : <select name="target_kind" value={props.kind} onChange={(event) => props.onKindChange(event.target.value as TargetKind)}>{props.availableKinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select>}</label>
      <label><span className="inline-flex items-center gap-1" title="IP-Adresse oder DNS-Name, unter dem der Worker das Ziel erreicht.">Adresse</span><input name="target_address" autoComplete="url" value={props.address} onChange={(event) => props.onAddressChange(event.target.value)} placeholder="IP oder DNS-Name …" required /></label>
      {selectedKind.transport === "ssh" && <label><span className="inline-flex items-center gap-1" title="Benutzername für die SSH-Verbindung zu diesem Ziel.">SSH-Benutzer</span><input name="ssh_user" autoComplete="username" value={props.sshUser} onChange={(event) => props.onSshUserChange(event.target.value)} placeholder="z. B. root oder lxcup …" required /></label>}
      <SecretSelect label="Deployment-Secret" title="Zugangsdaten für die Verbindung zum Ziel, zum Beispiel ein SSH-Passwort oder ein SSH-Private-Key." value={props.credentialSecret} options={activeSecrets} onChange={props.onCredentialChange} onNew={() => { props.onSecretForChange("credential"); props.onSecretKindChange(selectedKind.transport === "ssh" ? "ssh_password" : "generic"); }} />
      {selectedKind.transport === "ssh" && <SecretSelect label="SSH-Host-Fingerprint" title="Bekannter SSH-Host-Fingerprint als known_hosts-Datei." value={props.knownHostsSecret} options={activeSecrets.filter((item) => item.metadata.metadata.kind === "ssh_known_hosts")} onChange={props.onKnownHostsChange} onNew={() => { props.onSecretForChange("known_hosts"); props.onSecretKindChange("ssh_known_hosts"); }} emptyLabel="Known-Hosts-Secret auswählen" />}
      <SecretSelect label="Agent-Token" title="Geheimer Token, mit dem sich der installierte lxcup-Agent beim Controller authentifiziert." value={props.agentSecret} options={activeSecrets} onChange={props.onAgentChange} onNew={() => { props.onSecretForChange("agent"); props.onSecretKindChange("agent_token"); }} />
      <label><span className="inline-flex items-center gap-1" title="Verbindungsprotokoll, das automatisch aus dem Zieltyp abgeleitet wird.">Transport</span><input name="transport" value={selectedKind.transport.toUpperCase()} readOnly /></label>
    </div>
    {newSecretFor && <div className="col-span-full"><InlineSecretEditor newSecretFor={newSecretFor} name={newSecretName} kind={newSecretKind} value={newSecretValue} pending={inlineSecretPending} error={inlineSecretError} onCancel={() => props.onSecretForChange(null)} onNameChange={props.onSecretNameChange} onKindChange={props.onSecretKindChange} onValueChange={props.onSecretValueChange} onGenerate={() => props.onSecretValueChange(generateSecretValue())} onCreate={props.onCreateSecret} /></div>}
    {props.onboardingAvailable ? <label className="col-span-full flex items-start gap-2 border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-sm font-medium"><input className="mt-0.5 shrink-0" type="checkbox" aria-label="Onboarding direkt starten" checked={props.startOnboarding} onChange={(event) => props.onStartOnboardingChange(event.target.checked)} /><span><strong className="block">Onboarding direkt starten</strong><span className="text-xs font-normal text-[var(--muted)]">Agent installieren und nach erfolgreichem Heartbeat einen Healthcheck ausführen.</span></span></label> : null}
    <div className="col-span-full grid gap-2 border-t border-[var(--line)] pt-4">
      <button className="mx-auto inline-flex min-h-10 w-full max-w-[15rem] items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 py-2 text-sm font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={createPending || props.credentialSecret === "" || props.agentSecret === "" || (selectedKind.transport === "ssh" && props.knownHostsSecret === "")}>{createPending ? "Wird angelegt…" : props.submitLabel}</button>
      {createError && <p className="text-center font-semibold text-[var(--error)]" role="alert">{createError}</p>}
    </div>
  </form>;
}

function SecretSelect({ label, title, value, options, onChange, onNew, emptyLabel = "Secret auswählen" }: Readonly<{ label: string; title: string; value: string; options: SecretMetadata[]; onChange: (value: string) => void; onNew: () => void; emptyLabel?: string }>) {
  return <label><span className="inline-flex items-center gap-1" title={title}>{label}</span><div className="flex items-stretch gap-2"><select value={value} onChange={(event) => onChange(event.target.value)} required><option value="">{emptyLabel}</option>{options.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select><button className="inline-flex min-h-9 shrink-0 items-center justify-center gap-1 border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onNew}>＋ Neu</button></div></label>;
}

function InlineSecretEditor({ newSecretFor, name, kind, value, pending, error, onCancel, onNameChange, onKindChange, onValueChange, onGenerate, onCreate }: Readonly<{ newSecretFor: "credential" | "known_hosts" | "agent"; name: string; kind: SecretKind; value: string; pending: boolean; error?: string; onCancel: () => void; onNameChange: (value: string) => void; onKindChange: (value: SecretKind) => void; onValueChange: (value: string) => void; onGenerate: () => void; onCreate: () => void }>) {
  return <fieldset className="my-3 grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-3"><legend>Neues {secretPurposeLabel(newSecretFor)}</legend><button className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" type="button" onClick={onCancel}>Abbrechen</button><div className="grid grid-cols-1 gap-3 sm:grid-cols-2"><label><span>Name</span><input value={name} onChange={(event) => onNameChange(event.target.value)} placeholder="z. B. lxcup-test-ssh" autoComplete="off" /></label><label><span>Typ</span><select value={kind} onChange={(event) => onKindChange(event.target.value as SecretKind)}><option value="ssh_password">SSH Passwort</option><option value="ssh_private_key">SSH Private Key</option><option value="ssh_known_hosts">SSH Known Hosts</option><option value="agent_token">Agent-Token</option><option value="generic">Allgemein</option></select></label><label><span>Wert</span><input type="password" value={value} onChange={(event) => onValueChange(event.target.value)} autoComplete="new-password" placeholder="Wert eingeben oder erzeugen" /></label><button className="inline-flex self-end items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onGenerate}>Wert erzeugen</button></div><button className="inline-flex justify-self-start items-center justify-center border border-lxcup-primary bg-lxcup-primary px-2.5 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={pending || name.trim() === "" || value === ""} onClick={onCreate}>{pending ? "Speichert…" : "Secret erstellen und auswählen"}</button>{error && <p className="font-semibold text-[var(--error)]" role="alert">{error}</p>}</fieldset>;
}

function secretPurposeLabel(value: "credential" | "known_hosts" | "agent") {
  if (value === "credential") return "Deployment-Secret";
  if (value === "known_hosts") return "Known-Hosts-Secret";
  return "Agent-Token";
}

type JobMutationView = Readonly<{ isPending: boolean; data?: { id: string }; error: unknown }>;

function OnboardingActivities({ deployment, health, inventory }: Readonly<{ deployment: JobMutationView; health: JobMutationView; inventory: JobMutationView }>) {
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Onboarding-Aktivitäten</h2><p className="text-[var(--muted)]">Jeder Schritt wird als eigener Workflow mit eigenem Protokoll geführt.</p></div></div><div className="grid gap-2 md:grid-cols-2"><div><strong>1. Agent installieren</strong>{jobActivity(deployment, "wird erstellt…")}{mutationError(deployment.error)}</div><div><strong>2. Healthcheck</strong>{jobActivity(health, "wird gestartet…", "Startet nach erfolgreicher Agent-Installation und Heartbeat.")}{mutationError(health.error)}</div><div><strong>3. Paketinventar</strong>{jobActivity(inventory, "wird erfasst…", "Startet nach erfolgreichem Healthcheck.")}{mutationError(inventory.error)}</div></div></section>;
}

function jobActivity(job: JobMutationView, pendingLabel: string, idleLabel?: string) {
  if (job.isPending) return <span className="text-[var(--muted)]"> {pendingLabel}</span>;
  if (job.data) return <Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to={`/workflows/${job.data.id}`}>Protokoll öffnen →</Link>;
  return idleLabel ? <p className="text-[var(--muted)]">{idleLabel}</p> : null;
}

function mutationError(error: unknown) {
  return error instanceof Error ? <p className="font-semibold text-[var(--error)]">{error.message}</p> : null;
}

function TargetInventory({ targets, isLoading, jobs }: Readonly<{ targets: TargetDto[]; isLoading: boolean; jobs: AnsibleJobDto[] }>) {
  if (isLoading) return <p className="text-[var(--muted)]">Lade Zugangsprofile…</p>;
  if (targets.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Zugangsprofile für diese Ressourcenart angelegt.</p>;
  return <ul className="m-0 grid list-none gap-3 p-0" aria-label="Ressourcen">
    {targets.map((target) => <TargetInventoryCard key={target.id} target={target} jobs={jobs} />)}
  </ul>;
}

function TargetInventoryCard({ target, jobs }: Readonly<{ target: TargetDto; jobs: AnsibleJobDto[] }>) {
  const inventory = usePackageInventory(target.id);
  const telemetry = useTargetTelemetry(target.id);
  const latestSample = telemetry.data?.samples.at(-1);
  const telemetryTime = telemetry.data?.collected_at ?? latestSample?.collected_at;
  const telemetryStale = telemetryTime ? isTelemetryStale(telemetryTime) : false;
  const inventorySummary = targetInventorySummary(inventory);
  const telemetrySummary = targetTelemetrySummary(telemetry, telemetryTime, telemetryStale);

  return <li>
      <article className="border border-[var(--line)] bg-[var(--paper-muted)] p-4 transition-colors hover:border-[var(--primary)]">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 items-start gap-3">
            <span className="grid h-10 w-10 shrink-0 place-items-center border border-[var(--line)] bg-[var(--panel)] text-[11px] font-bold uppercase tracking-wide text-lxcup-primary" aria-label={targetKindLabel(target.kind)}>{targetKindShortLabel(target.kind)}</span>
            <div className="min-w-0">
              <Link className="break-words text-base font-semibold text-lxcup-primary hover:underline" to={`/targets/${target.id}`}>{target.name}</Link>
              <p className="mb-0 mt-1 break-all text-sm text-[var(--muted)]">{target.address}</p>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="border border-[var(--line)] bg-[var(--panel)] px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">{target.transport}</span>
            <span className={cn("inline-flex items-center px-2 py-1 text-xs font-bold", targetStateStatusClass(target.state))}>{targetStateLabel(target.state)}</span>
          </div>
        </div>

        <div className="mt-4 grid gap-4 border-t border-[var(--line)] pt-3 md:grid-cols-[minmax(8rem,0.7fr)_minmax(0,2fr)] xl:grid-cols-[minmax(8rem,0.7fr)_minmax(0,2fr)_minmax(14rem,1fr)] xl:items-center">
          <div>
            <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Installierte Agent-Version</p>
            <p className="mb-0 text-sm font-semibold" title="Wird vom letzten authentifizierten Heartbeat des Zielsystems gemeldet.">{target.agent_version ? `v${target.agent_version}` : <span className="font-normal text-[var(--muted)]">Noch keine Meldung</span>}</p>
          </div>
          <div>
            <p className="mb-2 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Onboarding</p>
            <TargetOnboardingProtocols target={target} jobs={jobs} />
          </div>
          <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-1">
            <TargetSignalLink to={`/targets/${target.id}/packages`} label="Paketinventar" value={inventorySummary} status={inventorySignalStatus(inventory)} accessibleName={`Paketinventar für ${target.name}`} />
            <TargetSignalLink to={`/targets/${target.id}`} label="Systemauslastung" value={telemetrySummary} status={telemetrySignalStatus(telemetry, telemetryStale, telemetryTime)} accessibleName={`Systemauslastung für ${target.name}`} />
          </div>
        </div>
      </article>
    </li>;
}

function targetInventorySummary(inventory: ReturnType<typeof usePackageInventory>) {
  if (inventory.isLoading) return "Wird geladen…";
  if (inventory.error) return "Fehler beim Laden";
  if (inventory.data?.status !== "complete") return "Noch nicht erhoben";
  const collectedAt = inventory.data.collected_at
    ? new Date(inventory.data.collected_at).toLocaleString()
    : "Zeitpunkt unbekannt";
  return `${inventory.data.packages.length} Pakete · ${collectedAt}`;
}

function targetTelemetrySummary(telemetry: ReturnType<typeof useTargetTelemetry>, collectedAt: string | undefined, stale: boolean) {
  if (telemetry.isLoading) return "Wird geladen…";
  if (telemetry.error) return "Fehler beim Laden";
  if (collectedAt === undefined) return "Noch keine Telemetrie";
  return `${stale ? "Veraltet" : "Aktuell"} · ${new Date(collectedAt).toLocaleString()}`;
}

type SignalStatus = "error" | "success" | "warning" | "neutral";

function inventorySignalStatus(inventory: ReturnType<typeof usePackageInventory>): SignalStatus {
  if (inventory.error) return "error";
  if (inventory.data?.status === "complete") return "success";
  return "neutral";
}

function telemetrySignalStatus(telemetry: ReturnType<typeof useTargetTelemetry>, stale: boolean, collectedAt: string | undefined): SignalStatus {
  if (telemetry.error) return "error";
  if (stale) return "warning";
  if (collectedAt !== undefined) return "success";
  return "neutral";
}

function TargetSignalLink({ to, label, value, status, accessibleName }: Readonly<{ to: string; label: string; value: string; status: "error" | "success" | "warning" | "neutral"; accessibleName: string }>) {
  const statusClass = targetSignalStatusClass(status);
  return <Link className="grid min-w-0 gap-1 border border-[var(--line)] bg-[var(--panel)] px-3 py-2 hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" to={to} aria-label={accessibleName}>
    <span className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">{label}</span>
    <span className={cn("truncate border px-2 py-1 text-xs font-semibold", statusClass)} title={value}>{value}</span>
  </Link>;
}

function targetSignalStatusClass(status: SignalStatus) {
  switch (status) {
    case "success": return "border-[var(--success)]/40 bg-[var(--success-soft)] text-[var(--success)]";
    case "warning": return "border-[var(--warning)]/40 bg-[var(--warning-soft)] text-[var(--warning)]";
    case "error": return "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]";
    case "neutral": return "border-[var(--line)] bg-[var(--panel)] text-[var(--muted)]";
  }
}

function targetKindShortLabel(kind: TargetKind) {
  if (kind === "linux_server") return "Linux";
  if (kind === "windows_server") return "Win";
  return "LXC";
}

function targetKindLabel(kind: TargetKind) {
  if (kind === "linux_server") return "Linux-Server";
  if (kind === "windows_server") return "Windows-Server";
  return "LXC-Container";
}

function targetStateStatusClass(state: TargetState) {
  if (state === "managed") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (state === "disabled") return "bg-[var(--paper-muted)] text-[var(--muted)]";
  return "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function TargetOnboardingProtocols({ target, jobs }: Readonly<{ target: import("../api").TargetDto; jobs: import("../api").AnsibleJobDto[] }>) {
  const targetJobs = jobs.filter((job) => "target" in job.target && job.target.target === target.id);
  const deployment = targetJobs.find((job) => job.operation === "deploy_agent");
  const laterJobs = deployment ? targetJobs.filter((job) => job.created_at >= deployment.created_at) : [];
  const health = laterJobs.find((job) => job.operation === "health_check");
  const inventory = laterJobs.find((job) => job.operation === "collect_package_inventory");
  const steps = [
    ["Agent", deployment, "Nicht gestartet"],
    ["Healthcheck", health, "Ausstehend"],
    ["Paketinventar", inventory, "Ausstehend"],
  ] as const;
  return <ol className="m-0 flex flex-wrap gap-2 p-0" aria-label={`Onboarding-Status für ${target.name}`}>
    {steps.map(([label, job, pendingLabel]) => <li className="list-none" key={label}>
      {job ? <Link className={cn("inline-flex items-center border border-transparent px-2 py-1 text-xs font-semibold hover:border-[var(--primary)] hover:underline", jobStatusBadgeClass(job.status))} to={`/workflows/${job.id}`} title={`${label}: ${jobStatusLabel(job.status)}`}>{label} · {jobStatusLabel(job.status)}</Link> : <span className="inline-flex items-center border border-[var(--line)] px-2 py-1 text-xs text-[var(--muted)]">{label} · {pendingLabel}</span>}
    </li>)}
  </ol>;
}

function targetStateClass(state: TargetState) {
  if (state === "managed") return "success";
  if (state === "disabled") return "neutral";
  return "pending";
}

function targetStateLabel(state: TargetState) {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Pending";
}
