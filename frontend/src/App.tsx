import { useEffect, useState } from "react";
import { Link, NavLink, Route, Routes, useLocation } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError, apiClient, type AnsibleJobDto, type ApiEvent, type AuthRole, type AuthSession, type ContainerDto, type TargetDto } from "./api";
import { queryKeys, useAnsibleJobs, useContainers, useTargets, useWorkerAvailability } from "./queries";
import { Dashboard } from "./pages/Dashboard";
import { WorkflowsPage } from "./pages/WorkflowsPage";
import { WorkflowDetailPage } from "./pages/WorkflowDetailPage";
import { SecretsPage } from "./pages/SecretsPage";
import { ContainerDetailPage } from "./pages/ContainerDetailPage";
import { EnrollmentPage } from "./pages/EnrollmentPage";
import { TargetsPage } from "./pages/TargetsPage";
import { PackageInventoryPage } from "./pages/PackageInventoryPage";
import { TargetDetailPage } from "./pages/TargetDetailPage";
import { DockerPage } from "./pages/DockerPage";
import { SchedulesPage } from "./pages/SchedulesPage";
import { UpdatePoliciesPage } from "./pages/UpdatePoliciesPage";
import { TaskMonitor, type GlobalEvent } from "./components/TaskMonitor";
import { cn } from "./classnames";
import { loadActivityEvents, saveActivityEvents, syncJobActivity } from "./activityPersistence";
import { jobStatusBadgeClass, jobStatusLabel } from "./jobStatus";

const ACTIVITY_SIDEBAR_STORAGE_KEY = "lxcup-activity-sidebar-open";

export default function App() {
  const [authSession, setAuthSession] = useState<AuthSession | null>(null);
  const [authReady, setAuthReady] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);

  useEffect(() => {
    apiClient.setUnauthorizedHandler(() => {
      apiClient.setCredentials(null);
      setAuthSession(null);
      setAuthError(null);
    });
    return () => apiClient.setUnauthorizedHandler(null);
  }, []);

  useEffect(() => {
    let active = true;
    void apiClient.getSession().then((session) => {
      if (active) { apiClient.setCredentials(null, session.role); setAuthSession(session); setAuthReady(true); }
    }).catch((error: unknown) => {
      if (!active) return;
      setAuthError(error instanceof ApiError && error.status === 401 ? null : "Die API-Sitzung konnte nicht geprüft werden.");
      setAuthReady(true);
    });
    return () => { active = false; };
  }, []);

  const login = async (token: string) => {
    apiClient.setCredentials(token);
    try {
      const session = await apiClient.getSession();
      apiClient.setCredentials(token, session.role);
      setAuthSession(session);
      setAuthError(null);
      return true;
    } catch {
      apiClient.setCredentials(null);
      return false;
    }
  };

  const logout = async () => {
    try { await apiClient.logout(); } catch { /* Local logout still clears the credential. */ }
    apiClient.setCredentials(null);
    setAuthSession(null);
    setAuthError(null);
  };

  if (!authReady) return <AuthMessage message="Sitzung wird geprüft …" />;
  if (authError) return <AuthMessage message={authError} />;
  if (!authSession) return <LoginScreen onLogin={login} />;
  return <AuthenticatedApp session={authSession} onLogout={logout} />;
}

function AuthenticatedApp({ session, onLogout }: Readonly<{ session: AuthSession; onLogout: () => void }>) {
  const queryClient = useQueryClient();
  const location = useLocation();
  const jobs = useAnsibleJobs();
  const targets = useTargets();
  const containers = useContainers();
  const [connectionState, setConnectionState] = useState("verbunden");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [events, setEvents] = useState<GlobalEvent[]>(loadActivityEvents);
  const [activityOpen, setActivityOpen] = useState(() => globalThis.localStorage?.getItem(ACTIVITY_SIDEBAR_STORAGE_KEY) !== "false");
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    const stored = globalThis.localStorage?.getItem("lxcup-theme-v2");
    return stored === "dark" ? "dark" : "light";
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    globalThis.localStorage?.setItem("lxcup-theme-v2", theme);
  }, [theme]);

  useEffect(() => saveActivityEvents(events), [events]);

  useEffect(() => {
    globalThis.localStorage?.setItem(ACTIVITY_SIDEBAR_STORAGE_KEY, String(activityOpen));
  }, [activityOpen]);

  useEffect(() => {
    if (!jobs.isSuccess) return;
    setEvents((current) => syncJobActivity(current, jobs.data, targets.data ?? [], containers.data ?? []));
  }, [jobs.data, jobs.isSuccess, targets.data, containers.data]);

  useEffect(() => {
    return apiClient.subscribe(
      (event: ApiEvent) => {
        setConnectionState("verbunden");
        setStreamError(null);
        setEvents((current) => appendActivityEvent(current, event));
        if (event.type === "Error") setStreamError(event.payload.message);
        if (event.type === "Status") {
          const queryKey = eventQueryKey(event.payload.resource);
          void queryClient.invalidateQueries({ queryKey });
        }
      },
      () => { setConnectionState("wiederverbinden"); setStreamError("Echtzeitverbindung unterbrochen. Der Browser versucht die Verbindung erneut."); },
    );
  }, [queryClient]);

  return (
    <div className="grid h-dvh min-h-0 grid-cols-[270px_minmax(0,1fr)] overflow-hidden max-[720px]:grid-cols-1">
      <aside className="sticky top-0 flex h-dvh min-h-0 flex-col gap-5 overflow-y-auto border-r border-slate-700 bg-[var(--nav-bg)] px-3 py-4 text-sm text-[var(--nav-text)] [scrollbar-color:var(--line)_transparent] [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-[var(--line)] [&::-webkit-scrollbar-track]:bg-transparent max-[720px]:static max-[720px]:h-auto max-[720px]:max-h-[32dvh] max-[720px]:border-b">
        <Link className="group flex items-center gap-3 border-b border-slate-700 px-1 pb-4" to="/" aria-label="lxcup Übersicht">
          <span className="grid h-10 w-10 shrink-0 place-items-center border border-blue-400/30 bg-lxcup-primary text-[11px] font-extrabold tracking-tight text-white shadow-sm shadow-blue-950/40" aria-hidden="true">LX</span>
          <span className="min-w-0"><strong className="block text-base font-semibold leading-tight tracking-tight text-white">lxcup</strong><small className="mt-1 block text-[9px] font-bold uppercase tracking-[0.16em] text-slate-400">Control plane</small></span>
          <span className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-lxcup-primary opacity-80" aria-hidden="true" />
        </Link>
        <div className="flex items-center justify-between px-2">
          <span className="text-[9px] font-bold uppercase tracking-[0.16em] text-slate-500">Datacenter control</span>
          <span className="border border-slate-700 px-1.5 py-0.5 text-[8px] font-bold uppercase tracking-wider text-slate-500">Self-hosted</span>
        </div>
        <nav className="grid gap-5" aria-label="Hauptnavigation">
          <NavigationSection title="Übersicht">
            <NavigationLink to="/" label="Übersicht" icon="overview" />
          </NavigationSection>
          <NavigationSection title="Ressourcen">
            <NavigationLink to="/servers" label="Server" icon="server" />
            <NavigationLink to="/containers" label="LXC-Container" icon="lxc" />
            <NavigationLink to="/docker" label="Docker-Container" icon="docker" />
            <NavigationLink to="/windows" label="Windows" icon="windows" />
          </NavigationSection>
          <NavigationSection title="Automatisierung">
            <NavigationLink to="/workflows" label="Tasks & Workflows" icon="workflows" />
            <NavigationLink to="/schedules" label="Zeitpläne" icon="schedules" />
            <NavigationLink to="/update-policies" label="Update-Policies" icon="policies" />
          </NavigationSection>
          <NavigationSection title="System">
            <NavigationLink to="/secrets" label="Secrets" icon="secrets" />
          </NavigationSection>
        </nav>
        <div className="mt-auto flex items-center gap-3 border border-slate-700 bg-[var(--nav-surface)] px-3 py-3 text-xs" aria-live="polite">
          <span className={cn("relative inline-flex h-2.5 w-2.5 shrink-0 rounded-full", connectionState === "verbunden" ? "bg-emerald-400" : "bg-amber-400")}><span className={cn("absolute inset-0 animate-ping rounded-full opacity-30 motion-reduce:animate-none", connectionState === "verbunden" ? "bg-emerald-400" : "bg-amber-400")} /></span>
          <span className="min-w-0"><strong className="block text-[9px] uppercase tracking-[0.14em] text-slate-300">Echtzeitverbindung</strong><small className="mt-1 block truncate text-[11px] text-slate-400">SSE · {connectionState}</small></span>
          <span className={cn("ml-auto h-1.5 w-1.5 shrink-0 rounded-full", connectionState === "verbunden" ? "bg-emerald-400" : "bg-amber-400")} aria-hidden="true" />
        </div>
      </aside>
      <main className="flex h-dvh min-h-0 min-w-0 flex-col overflow-hidden bg-[var(--paper)]">
        <header className="relative z-[60] flex h-14 shrink-0 items-center justify-between gap-4 border-b border-[var(--line)] bg-[var(--panel)] px-5 shadow-[0_2px_10px_rgb(0_0_0/0.08)] max-[720px]:gap-2 max-[720px]:px-3">
          <nav className="flex min-w-0 items-center gap-3 overflow-hidden whitespace-nowrap" aria-label="Brotkrumennavigation">
            <span className="border border-[var(--line)] bg-[var(--primary-soft)] px-2 py-1 text-[9px] font-extrabold uppercase tracking-[0.14em] text-lxcup-primary max-[720px]:hidden">Datacenter</span>
            <Link className="truncate text-xs font-semibold tracking-wide text-[var(--muted)] transition-colors hover:text-lxcup-primary max-[720px]:hidden" to="/">lxcup-control</Link>
            <span className="text-xs text-[var(--line)] max-[720px]:hidden" aria-hidden="true">/</span>
            <span className="truncate text-sm font-semibold text-[var(--ink)]" aria-current="page">{headerPageTitle(location.pathname)}</span>
          </nav>
          <div className="flex shrink-0 items-center gap-2 max-[720px]:gap-1">
            <NotificationCenter targets={targets.data ?? []} containers={containers.data ?? []} />
            <span className="inline-flex min-h-9 items-center border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 text-[11px] font-semibold text-[var(--muted)] max-[720px]:hidden" title={session.expires_in_seconds === null ? "Authentifizierung in dieser Umgebung deaktiviert" : `Token läuft in ${formatDuration(session.expires_in_seconds)} ab`}>{roleLabel(session.role)}</span>
            <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary max-[720px]:px-2" type="button" onClick={onLogout}>Abmelden</button>
            <button className="inline-flex h-9 w-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] text-sm text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" type="button" onClick={() => setTheme(toggleTheme)} aria-label="Theme wechseln" title="Theme wechseln">{theme === "dark" ? "☼" : "☾"}</button>
          </div>
        </header>
        <div className={activityOpen ? "grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(0,1fr)_24rem] items-stretch gap-3 overflow-hidden px-4 py-3 transition-[grid-template-columns] duration-300 ease-in-out motion-reduce:transition-none sm:px-6 max-[1000px]:grid-cols-[minmax(0,1fr)]" : "grid min-h-0 min-w-0 flex-1 grid-cols-1 items-stretch overflow-hidden px-4 py-3 transition-[grid-template-columns] duration-300 ease-in-out motion-reduce:transition-none sm:px-6"}>
          <div className="min-h-0 min-w-0 overflow-x-hidden overflow-y-auto [scrollbar-color:var(--line)_transparent] [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-[var(--line)] [&::-webkit-scrollbar-track]:bg-transparent">
            {session.role === "viewer" ? <div className="border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]">Viewer-Zugriff: Änderungen und Workflow-Ausführungen sind deaktiviert.</div> : null}
            {session.role === "operator" ? <div className="border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]">Operator-Zugriff: Ziele und freigegebene Workflows verwalten; Secret-Verwaltung und destruktive Aktionen erfordern Admin.</div> : null}
            {streamError ? <div className="mb-3 border border-[#edb7c0] bg-[var(--error-soft)] px-3 py-2 font-medium text-[var(--error)]" role="alert">{streamError}</div> : null}
            <WorkerAvailabilityBanner />
            <Routes>
              <Route path="/" element={<Dashboard />} />
              <Route path="/servers" element={<TargetsPage area="linux_server" />} />
              <Route path="/windows" element={<TargetsPage area="windows_server" />} />
              <Route path="/containers" element={<TargetsPage area="lxc" />} />
              <Route path="/targets" element={<TargetsPage />} />
              <Route path="/targets/:targetId" element={<TargetDetailPage />} />
              <Route path="/targets/:targetId/packages" element={<PackageInventoryPage />} />
              <Route path="/containers/:containerId" element={<ContainerDetailPage />} />
              <Route path="/docker" element={<DockerPage />} />
              <Route path="/enrollments/new" element={<EnrollmentPage />} />
              <Route path="/workflows" element={<WorkflowsPage />} />
              <Route path="/schedules" element={<SchedulesPage />} />
              <Route path="/update-policies" element={<UpdatePoliciesPage />} />
              <Route path="/workflows/:jobId" element={<WorkflowDetailPage />} />
              <Route path="/secrets" element={<SecretsPage />} />
              <Route path="*" element={<NotFound />} />
            </Routes>
          </div>
          <TaskMonitor
            events={events}
            onClear={() => setEvents([])}
            collapsed={!activityOpen}
            onToggle={() => setActivityOpen((open) => !open)}
          />
        </div>
      </main>
    </div>
  );
}

function appendActivityEvent(current: GlobalEvent[], event: ApiEvent): GlobalEvent[] {
  const isJobUpdate = event.type === "Status" && event.payload.resource === "ansible_job";
  const previous = isJobUpdate ? current.filter((item) => !isMatchingJobEvent(item, event)) : current;
  return [{ id: crypto.randomUUID(), event, receivedAt: new Date().toISOString() }, ...previous].slice(0, 40);
}

function isMatchingJobEvent(item: GlobalEvent, event: ApiEvent): boolean {
  return event.type === "Status" && item.event.type === "Status" && item.event.payload.resource === "ansible_job"
    && item.event.payload.resource_id === event.payload.resource_id;
}

function LoginScreen({ onLogin }: Readonly<{ onLogin: (token: string) => Promise<boolean> }>) {
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const submit = async (event: React.SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    setPending(true);
    setError(null);
    const accepted = await onLogin(token.trim());
    setPending(false);
    if (!accepted) { setToken(""); setError("Token ungültig oder abgelaufen. Bitte erneut versuchen."); }
  };
  return <main className="grid min-h-screen place-items-center bg-[var(--paper)] p-4 text-[var(--ink)]"><form className="grid w-full max-w-sm gap-3 border border-[var(--line)] bg-[var(--panel)] p-5 shadow-xl" onSubmit={(event) => void submit(event)}><div><h1 className="m-0 text-xl font-semibold">Bei lxcup anmelden</h1><p className="mt-1 text-sm text-[var(--muted)]">Gib das Viewer-, Operator- oder Admin-Token ein.</p></div><label className="grid gap-1 text-xs font-semibold">API-Token<input autoComplete="off" autoFocus className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" type="password" value={token} onChange={(event) => setToken(event.target.value)} required /></label>{error ? <p className="m-0 text-sm text-[var(--error)]" role="alert">{error}</p> : null}<button className="inline-flex justify-self-start items-center justify-center border border-lxcup-primary bg-lxcup-primary px-2.5 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={pending || token.trim().length === 0}>{pending ? "Prüfe Token …" : "Anmelden"}</button><small className="text-[var(--muted)]">Das Token bleibt nur bis zum Schließen oder Neuladen dieses Tabs im Arbeitsspeicher.</small></form></main>;
}

function AuthMessage({ message }: Readonly<{ message: string }>) {
  return <main className="grid min-h-screen place-items-center bg-[var(--paper)] p-4 text-sm text-[var(--ink)]"><output>{message}</output></main>;
}

function roleLabel(role: AuthRole) {
  if (role === "admin") return "Admin";
  if (role === "operator") return "Operator";
  return "Viewer";
}
function toggleTheme(current: "dark" | "light"): "dark" | "light" { return current === "dark" ? "light" : "dark"; }
function formatDuration(seconds: number) { return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`; }

function WorkerAvailabilityBanner() {
  const availability = useWorkerAvailability();
  if (availability.data?.available !== false) return null;
  return <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1 border border-[#e7c47f] bg-[var(--warning-soft)] px-3 py-2 text-sm text-[var(--warning)]" role="alert"><strong>Ansible-Worker nicht verfügbar</strong><span>Neue Workflows bleiben eingereiht, bis ein Worker wieder aktiv ist.</span></div>;
}

function NotificationCenter({ targets, containers }: Readonly<{ targets: TargetDto[]; containers: ContainerDto[] }>) {
  const jobs = useAnsibleJobs();
  const failed = (jobs.data ?? []).filter((job) => job.status === "failed" || job.status === "reconcile_required");
  const [open, setOpen] = useState(false);
  const [read, setRead] = useState<string[]>(() => JSON.parse(globalThis.localStorage?.getItem("lxcup-read-notifications") ?? "[]") as string[]);
  const unread = failed.filter((job) => read.includes(job.id) === false);
  const saveRead = (next: string[]) => {
    globalThis.localStorage?.setItem("lxcup-read-notifications", JSON.stringify(next));
    setRead(next);
  };
  const markRead = (id: string) => saveRead(read.includes(id) ? read : [...read, id]);
  const markAllRead = () => saveRead([...new Set([...read, ...failed.map((job) => job.id)])]);
  return <div className="relative"><button className="relative inline-flex h-9 w-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] text-sm text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" type="button" aria-label="Benachrichtigungen" aria-expanded={open} onClick={() => setOpen((value) => !value)} title="Benachrichtigungen">🔔{unread.length ? <span className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-600 px-1 text-[10px] text-white">{unread.length}</span> : null}</button>{open ? <div className="absolute right-0 top-10 z-20 w-[min(24rem,calc(100vw-1rem))] overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)] shadow-xl"><div className="flex items-center justify-between gap-3 border-b border-[var(--line)] px-3 py-2.5"><div><strong className="block">Benachrichtigungen</strong><span className="text-xs text-[var(--muted)]">{failed.length} fehlgeschlagene Workflows</span></div><button className="text-xs font-semibold text-lxcup-primary hover:underline disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={unread.length === 0} onClick={markAllRead}>Alle als gelesen markieren</button></div>{notificationBody(failed, markRead, targets, containers)}</div> : null}</div>;
}

function eventQueryKey(resource: string) {
  if (resource === "target") return queryKeys.targets;
  if (resource === "ansible_job") return queryKeys.ansibleJobs;
  return queryKeys.containers;
}

function headerPageTitle(pathname: string) {
  if (pathname === "/") return "Übersicht";
  if (pathname === "/servers") return "Linux-Server";
  if (pathname === "/containers") return "LXC-Container";
  if (pathname.startsWith("/containers/")) return "LXC-Details";
  if (pathname === "/docker") return "Docker-Container";
  if (pathname === "/windows") return "Windows-Systeme";
  if (pathname === "/targets") return "Ziele";
  if (pathname.endsWith("/packages")) return "Paketinventar";
  if (pathname.startsWith("/targets/")) return "Ziel-Details";
  if (pathname === "/enrollments/new") return "Ziel einbinden";
  if (pathname === "/workflows") return "Tasks & Workflows";
  if (pathname.startsWith("/workflows/")) return "Workflow-Protokoll";
  if (pathname === "/schedules") return "Zeitpläne";
  if (pathname === "/update-policies") return "Update-Policies";
  if (pathname === "/secrets") return "Secrets";
  return "Seite nicht gefunden";
}

function notificationBody(failed: AnsibleJobDto[] | undefined, markRead: (id: string) => void, targets: TargetDto[], containers: ContainerDto[]) {
  if (failed === undefined || failed.length === 0) return <p className="m-0 px-3 py-4 text-sm text-[var(--muted)]">Keine fehlgeschlagenen Jobs.</p>;
  return <div className="max-h-[min(32rem,calc(100dvh-5rem))] overflow-y-auto">
    {failed.slice(0, 8).map((job) => {
      const resource = notificationResource(job, targets, containers);
      return <Link className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 border-b border-[var(--line)] px-3 py-3 text-xs transition-colors last:border-0 hover:bg-[var(--primary-soft)]" key={job.id} to={`/workflows/${job.id}`} onClick={() => markRead(job.id)}>
        <div className="flex min-w-0 items-center gap-2"><span className={cn("inline-flex shrink-0 items-center px-2 py-1 text-[10px] font-bold", jobStatusBadgeClass(job.status))}>{jobStatusLabel(job.status)}</span><strong className="truncate text-sm">{notificationOperationLabel(job.operation)}</strong></div>
        <time className="whitespace-nowrap text-[10px] text-[var(--muted)]">{new Date(job.updated_at).toLocaleString()}</time>
        <div className="col-span-2 grid min-w-0 gap-0.5 border-l-2 border-l-lxcup-primary pl-2.5">
          <span className="text-[10px] font-bold uppercase tracking-wide text-[var(--muted)]">Ressource</span>
          <strong className="truncate text-sm font-semibold">{resource.name}</strong>
          {resource.detail ? <span className="truncate text-xs text-[var(--muted)]">{resource.detail}</span> : null}
        </div>
        <span className="text-[10px] text-[var(--muted)]" title={job.id}>Job · {job.id.slice(0, 8)}</span>
        <span className="justify-self-end text-[10px] font-medium text-[var(--muted)]">{notificationModeLabel(job.mode)} <span aria-hidden="true">↗</span></span>
      </Link>;
    })}
  </div>;
}

function notificationResource(job: AnsibleJobDto, targets: TargetDto[], containers: ContainerDto[]) {
  if ("container" in job.target) {
    const containerId = (job.target as { container: number }).container;
    const container = containers.find((item) => item.id === containerId);
    return container ? { name: container.name, detail: `LXC · ${container.operating_system ?? "Linux"}` } : { name: `Container ${containerId}`, detail: "LXC-Container" };
  }
  if ("node" in job.target) return { name: `Node ${(job.target as { node: string }).node}`, detail: "Server-Node" };
  const targetId = (job.target as { target: string }).target;
  const target = targets.find((item) => item.id === targetId);
  return target ? { name: target.name, detail: `${target.kind} · ${target.address}` } : { name: `Ziel ${targetId.slice(0, 8)}`, detail: undefined };
}

function notificationOperationLabel(operation: string) {
  const labels: Record<string, string> = {
    deploy_agent: "Agent installieren",
    update_agent: "Agent aktualisieren",
    repair_agent: "Agent reparieren",
    update_packages: "Pakete aktualisieren",
    collect_package_inventory: "Paketinventar erfassen",
    health_check: "Healthcheck",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}

function notificationModeLabel(mode: string) {
  if (mode === "plan") return "Vorschau";
  if (mode === "reconcile") return "Abgleich";
  return mode === "apply" ? "Apply" : "Check";
}

function NotFound() {
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h1 className="m-0 text-xl font-semibold tracking-tight">Seite nicht gefunden</h1><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/">Zur Übersicht</Link></section>;
}

function NavigationSection({ title, children }: Readonly<{ title: string; children: React.ReactNode }>) {
  return <section className="grid gap-1" aria-label={title}>
    <h2 className="mb-1 px-2 text-[9px] font-bold uppercase tracking-[0.16em] text-slate-500 max-[720px]:hidden">{title}</h2>
    {children}
  </section>;
}

type NavigationIconName = "overview" | "server" | "lxc" | "docker" | "windows" | "workflows" | "schedules" | "policies" | "secrets";

function NavigationIcon({ name, active }: Readonly<{ name: NavigationIconName; active: boolean }>) {
  const shapes: Record<NavigationIconName, React.ReactNode> = {
    overview: <><rect x="3" y="3" width="5" height="5" rx="0.7" /><rect x="12" y="3" width="5" height="5" rx="0.7" /><rect x="3" y="12" width="5" height="5" rx="0.7" /><rect x="12" y="12" width="5" height="5" rx="0.7" /></>,
    server: <><rect x="3" y="3" width="14" height="5.5" rx="1" /><rect x="3" y="11.5" width="14" height="5.5" rx="1" /><path d="M6 5.75h.01M9 5.75h5M6 14.25h.01M9 14.25h5" /></>,
    lxc: <><path d="m10 2.75 6.75 3.8v6.9L10 17.25l-6.75-3.8v-6.9L10 2.75Z" /><path d="M3.5 6.75 10 10.5l6.5-3.75M10 10.5v6.25" /><path d="M7 4.5 13.5 8.25" /></>,
    docker: <><path d="M3 6.5h4v3H3zM8 6.5h4v3H8zM13 6.5h4v3h-4zM8 2.5h4v3H8zM8 10.5h4v3H8z" /><path d="M3.5 14.5c.7 1.3 2 2 4.1 2h4.8c2.9 0 4.7-1.4 5.1-4" /></>,
    windows: <path d="m3 4.5 6-1v6H3v-5ZM11 3.2l6-1v7.3h-6V3.2ZM3 11h6v6l-6-1v-5ZM11 11h6v7.3l-6-1V11Z" />,
    workflows: <><circle cx="5" cy="5" r="2" /><circle cx="15" cy="15" r="2" /><path d="M7 5h4a3 3 0 0 1 3 3v4M13 12l1 1 1-1M5 7v7a3 3 0 0 0 3 3h5" /></>,
    schedules: <><circle cx="10" cy="10" r="7" /><path d="M10 6v4l2.75 1.75M7 2v2M13 2v2" /></>,
    policies: <><path d="M10 2.5 16.5 5v4.6c0 4-2.7 6.7-6.5 8.9-3.8-2.2-6.5-4.9-6.5-8.9V5L10 2.5Z" /><path d="m7 10 2 2 4-4" /></>,
    secrets: <><circle cx="7" cy="11" r="3.5" /><path d="m10 8 6-6 2 2-1.5 1.5L18 7l-2 2-1.5-1.5-2 2M7 11h.01" /></>,
  };
  return <svg className={cn("h-[17px] w-[17px] shrink-0 transition-colors", active ? "text-lxcup-primary" : "text-slate-400 group-hover:text-slate-200")} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{shapes[name]}</svg>;
}

function NavigationLink({ to, label, icon }: Readonly<{ to: string; label: string; icon: NavigationIconName }>) {
  return <NavLink aria-label={label} title={label} className={({ isActive }) => cn("group flex min-h-9 items-center gap-3 border border-transparent border-l-2 px-3 py-2 text-[12px] font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-blue-400", isActive ? "border-[var(--line)] border-l-[var(--primary)] bg-[var(--nav-surface)] text-white" : "text-slate-300 hover:border-slate-700 hover:bg-[var(--nav-surface)] hover:text-white")} to={to}>{({ isActive }) => <><NavigationIcon name={icon} active={isActive} /><span className="truncate">{label}</span></>}</NavLink>;
}

export { DataTable } from "./components/DataTable";
