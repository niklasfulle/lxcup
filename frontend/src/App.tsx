import { useEffect, useMemo, useState } from "react";
import { Link, NavLink, Route, Routes } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { apiClient, type ApiEvent } from "./api";
import { queryKeys, useAnsibleJobs, useTargets, useWorkerAvailability } from "./queries";
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
import { TaskMonitor, type GlobalEvent } from "./components/TaskMonitor";
import { cn, ui } from "./ui";

export default function App() {
  const queryClient = useQueryClient();
  const [connectionState, setConnectionState] = useState("verbunden");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [events, setEvents] = useState<GlobalEvent[]>([]);
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    const stored = globalThis.localStorage?.getItem("lxcup-theme-v2");
    return stored === "dark" ? "dark" : "light";
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    globalThis.localStorage?.setItem("lxcup-theme-v2", theme);
  }, [theme]);

  useEffect(() => {
    return apiClient.subscribe(
      (event: ApiEvent) => {
        setConnectionState("verbunden");
        setStreamError(null);
        setEvents((current) => [{ id: crypto.randomUUID(), event, receivedAt: new Date().toISOString() }, ...current].slice(0, 40));
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
    <div className={ui.appShell}>
      <aside className={ui.sidebar}>
        <Link className={ui.brand} to="/" aria-label="lxcup Übersicht">
          <span className={ui.brandMark} aria-hidden="true">LX</span>
          <span><strong className="block text-lg font-semibold leading-none tracking-tight">lxcup</strong><small className="mt-1 block text-[10px] font-bold uppercase tracking-wider text-slate-400">CONTROL PLANE</small></span>
        </Link>
        <p className={ui.brandCaption}>Datacenter Control</p>
        <nav aria-label="Hauptnavigation">
          <span className={ui.navHeading}>Übersicht</span>
          <NavigationLink to="/">▣ Übersicht</NavigationLink>
          <span className={ui.navHeading}>Ressourcen</span>
          <NavigationLink to="/servers">▰ Server</NavigationLink><NavigationLink to="/containers">▤ LXC-Container</NavigationLink><NavigationLink to="/docker">◫ Docker-Container</NavigationLink><NavigationLink to="/windows">▣ Windows</NavigationLink>
          <span className={ui.navHeading}>Automatisierung</span><NavigationLink to="/workflows">↗ Tasks &amp; Workflows</NavigationLink><NavigationLink to="/schedules">◷ Zeitpläne</NavigationLink>
          <span className={ui.navHeading}>System</span><NavigationLink to="/secrets">⌘ Secrets</NavigationLink>
        </nav>
        <div className={ui.connectionIndicator} aria-live="polite">
          <span className={cn(ui.statusDot, connectionState === "verbunden" ? ui.statusDotOk : ui.statusDotWarn)} />
          <span><strong className="text-[10px] uppercase tracking-wider">LIVE BUS</strong><small className="block text-slate-400">SSE: {connectionState}</small></span>
        </div>
      </aside>
      <main className={ui.mainContent}>
        <header className={ui.topbar}>
          <div className={ui.topbarContext}><span className={ui.environmentLabel}>Datacenter</span><strong>lxcup-control</strong><span className={ui.muted}>/ Übersicht</span></div>
          <div className={ui.topbarActions}><NotificationCenter /><span className={ui.userPill}>Admin</span><button className={ui.button} type="button" onClick={() => setTheme((current) => current === "dark" ? "light" : "dark")} aria-label="Theme wechseln">{theme === "dark" ? "☼" : "☾"}</button></div>
        </header>
        <div className={ui.contentArea}>
          {streamError ? <div className={ui.apiAlert} role="alert">{streamError}</div> : null}
          <WorkerAvailabilityBanner />
          <TaskMonitor events={events} onClear={() => setEvents([])} />
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
            <Route path="/workflows/:jobId" element={<WorkflowDetailPage />} />
            <Route path="/secrets" element={<SecretsPage />} />
            <Route path="*" element={<NotFound />} />
          </Routes>
        </div>
      </main>
    </div>
  );
}

function WorkerAvailabilityBanner() {
  const availability = useWorkerAvailability();
  if (availability.data?.available !== false) return null;
  return <div className={ui.workerBanner} role="alert"><strong>Ansible-Worker nicht verfügbar</strong><span>Neue Workflows bleiben eingereiht, bis ein Worker wieder aktiv ist.</span></div>;
}

function NotificationCenter() {
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
  return <div className={ui.notificationCenter}><button className={cn(ui.button, ui.notificationButton)} type="button" aria-label="Benachrichtigungen" aria-expanded={open} onClick={() => setOpen((value) => !value)}>🔔{unread.length ? <span className={ui.notificationCount}>{unread.length}</span> : null}</button>{open ? <div className={ui.notificationPopover}><div className={ui.notificationHeading}><strong>Benachrichtigungen</strong><button className={ui.textLink} type="button" disabled={unread.length === 0} onClick={markAllRead}>Alle als gelesen markieren</button></div>{notificationBody(failed, markRead)}</div> : null}</div>;
}

function eventQueryKey(resource: string) {
  if (resource === "target") return queryKeys.targets;
  if (resource === "ansible_job") return queryKeys.ansibleJobs;
  return queryKeys.containers;
}

function notificationBody(failed: ReturnType<typeof useAnsibleJobs>["data"], markRead: (id: string) => void) {
  if (failed === undefined || failed.length === 0) return <p className={ui.muted}>Keine fehlgeschlagenen Jobs.</p>;
  return failed.slice(0, 8).map((job) => <Link className="grid gap-1 border-b border-[var(--line)] pb-2 text-xs last:border-0 last:pb-0" key={job.id} to={`/workflows/${job.id}`} onClick={() => markRead(job.id)}><span className={cn(ui.statusBadge, job.status === "failed" ? ui.statusNeutral : ui.statusPending)}>{job.status === "failed" ? "Fehlgeschlagen" : "Abgleich"}</span><span>{job.operation.replaceAll("_", " ")}</span><small className={ui.muted}>{new Date(job.updated_at).toLocaleString()}</small></Link>);
}

function NotFound() {
  return <section className={ui.panel}><h1 className="m-0 text-xl font-semibold tracking-tight">Seite nicht gefunden</h1><Link className={ui.textLink} to="/">Zur Übersicht</Link></section>;
}

export function ResourceSummary() {
  const targets = useTargets();
  const targetCount = useMemo(() => targets.data?.length ?? 0, [targets.data]);
  const connectedCount = useMemo(() => targets.data?.filter((target) => target.state === "managed").length ?? 0, [targets.data]);
  return <div className={ui.summaryGrid}><SummaryCard label="Verwaltete Ressourcen" value={targetCount} loading={targets.isLoading} /><SummaryCard label="Agenten verbunden" value={connectedCount} loading={targets.isLoading} /></div>;
}

function SummaryCard({ label, value, loading }: Readonly<{ label: string; value: number; loading: boolean }>) {
  return <article className={ui.summaryCard}><span className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">{label}</span><strong className="text-2xl font-semibold leading-none text-lxcup-primary">{loading ? "…" : value}</strong></article>;
}

function NavigationLink({ to, children }: Readonly<{ to: string; children: React.ReactNode }>) {
  const label = typeof children === "string" ? children.replace(/^[^ ]+\s+/, "") : undefined;
  return <NavLink aria-label={label} className={({ isActive }) => cn(ui.navLink, isActive && ui.navLinkActive)} to={to}>{children}</NavLink>;
}

export { DataTable } from "./components/DataTable";
