import { useEffect, useRef, useState } from "react";
import { Link, matchPath, NavLink, Route, Routes, useLocation } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError, apiClient, type AnsibleJobDto, type ApiEvent, type AuthRole, type AuthSession, type ContainerDto, type TargetDto, type TelemetryAlertDto, type WorkerAvailabilityDto } from "./api";
import { queryKeys, useAnsibleJobs, useContainers, useTargets, useTelemetryAlerts, useWorkerAvailability } from "./queries";
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
import UsersPage from "./pages/UsersPage";
import AuditLogPage from "./pages/AuditLogPage";
import { BackupsPage } from "./pages/BackupsPage";
import AccountPasswordPage from "./pages/AccountPasswordPage";
import { WikiPage } from "./pages/WikiPage";
import { ServerConnectionError } from "./components/ServerConnectionError";
import { checkInitialAuthSession } from "./authSession";
import { TaskMonitor, type GlobalEvent } from "./components/TaskMonitor";
import { PasswordInput } from "./components/PasswordInput";
import { cn } from "./classnames";
import { loadActivityEvents, saveActivityEvents, syncJobActivity } from "./activityPersistence";
import { jobStatusBadgeClass, jobStatusLabel } from "./jobStatus";
import frontendPackage from "../package.json";

const ACTIVITY_SIDEBAR_STORAGE_KEY = "lxcup-activity-sidebar-open";
const TELEMETRY_ALERT_HISTORY_STORAGE_KEY = "lxcup-telemetry-alert-history";
const SYSTEM_ALERT_HISTORY_STORAGE_KEY = "lxcup-system-alert-history";
const NOTIFICATION_READ_STORAGE_KEY = "lxcup-read-notifications";

type TelemetryAlertNotification = TelemetryAlertDto & { status: "active" | "resolved"; resolved_at?: string };
type SystemAlertNotification = { id: string; title: string; message: string; href: string; status: "active" | "resolved"; observed_at: string; resolved_at?: string };

export default function App() {
  const queryClient = useQueryClient();
  const location = useLocation();
  const [authSession, setAuthSession] = useState<AuthSession | null>(null);
  const [authReady, setAuthReady] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [accountAuth, setAccountAuth] = useState(false);
  const [authCheckAttempt, setAuthCheckAttempt] = useState(0);

  useEffect(() => {
    apiClient.setUnauthorizedHandler(() => {
      apiClient.setCredentials(null);
      queryClient.clear();
      setAuthSession(null);
      setAuthError(null);
    });
    return () => apiClient.setUnauthorizedHandler(null);
  }, [queryClient]);

  useEffect(() => {
    let active = true;
    void checkInitialAuthSession().then((result) => {
      if (!active) return;
      if (result.kind === "authenticated") {
        apiClient.setCredentials(null, result.session.role);
        setAuthSession(result.session);
      } else if (result.kind === "unauthenticated") {
        setAccountAuth(result.accountAuth);
      } else {
        setAuthError("Der lxcup-Server ist nicht erreichbar oder antwortet nicht korrekt.");
      }
      setAuthReady(true);
    });
    return () => { active = false; };
  }, [authCheckAttempt]);

  const retryAuthCheck = () => {
    setAuthError(null);
    setAuthReady(false);
    setAuthCheckAttempt((attempt) => attempt + 1);
  };

  const login = async (token: string) => {
    queryClient.clear();
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

  const loginAccount = async (username: string, password: string) => {
    queryClient.clear();
    try {
      await apiClient.login(username, password);
      const session = await apiClient.getSession();
      apiClient.setCredentials(null, session.role);
      setAuthSession(session);
      setAuthError(null);
      return true;
    } catch {
      apiClient.setCredentials(null);
      return false;
    }
  };

  const refreshSession = async () => {
    const session = await apiClient.getSession();
    setAuthSession(session);
  };

  const logout = async () => {
    try { await apiClient.logout(); } catch { /* Local logout still clears the credential. */ }
    apiClient.setCredentials(null);
    queryClient.clear();
    setAuthSession(null);
    setAuthError(null);
  };

  if (!authReady) return <AuthMessage message="Sitzung wird geprüft …" />;
  if (authError) return <ServerConnectionError message={authError} onRetry={retryAuthCheck} />;
  if (!authSession) {
    if (!isKnownApplicationPath(location.pathname)) return <NotFound />;
    return accountAuth
      ? <AccountLoginScreen onLogin={loginAccount} />
      : <LoginScreen onLogin={login} />;
  }
  if (authSession.must_change_password) {
    return <PasswordOnboardingScreen onComplete={() => void refreshSession()} />;
  }
  return <AuthenticatedApp session={authSession} onLogout={logout} />;
}

const APPLICATION_ROUTES = [
  "/",
  "/servers",
  "/windows",
  "/containers",
  "/docker",
  "/workflows",
  "/workflows/:jobId",
  "/schedules",
  "/update-policies",
  "/targets",
  "/targets/:targetId",
  "/targets/:targetId/packages",
  "/containers/:containerId",
  "/enrollments/new",
  "/secrets",
  "/account/security",
  "/admin/users",
  "/admin/audit",
  "/admin/backups",
  "/wiki",
];

function isKnownApplicationPath(pathname: string) {
  return APPLICATION_ROUTES.some((path) => matchPath({ path, end: true }, pathname) !== null);
}

function AuthenticatedApp({ session, onLogout }: Readonly<{ session: AuthSession; onLogout: () => void }>) {
  const queryClient = useQueryClient();
  const location = useLocation();
  const jobs = useAnsibleJobs();
  const targets = useTargets();
  const containers = useContainers();
  const [connectionState, setConnectionState] = useState("verbunden");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [events, setEvents] = useState<GlobalEvent[]>(() => loadActivityEvents().filter((item) => (
    session.role === "admin" || item.event.type !== "Status" || item.event.payload.resource !== "backup"
  )));
  const [activityOpen, setActivityOpen] = useState(() => globalThis.localStorage?.getItem(ACTIVITY_SIDEBAR_STORAGE_KEY) !== "false");
  const { theme, toggle } = useTheme();

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
        if (event.type === "Status" && event.payload.resource === "backup" && session.role !== "admin") return;
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
  }, [queryClient, session.role]);

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
          {session.role === "admin" ? <NavigationSection title="System">
            <NavigationLink to="/secrets" label="Secrets" icon="secrets" />
            <NavigationLink to="/admin/users" label="Benutzer" icon="users" />
            <NavigationLink to="/admin/audit" label="Aktivitätsprotokoll" icon="audit" />
            <NavigationLink to="/admin/backups" label="Backups" icon="backup" />
          </NavigationSection> : null}
          <NavigationSection title="Hilfe">
            <NavigationLink to="/wiki" label="Wiki" icon="help" />
          </NavigationSection>
        </nav>
        <div className="mt-auto flex items-center gap-3 border border-slate-700 bg-[var(--nav-surface)] px-3 py-3 text-xs" aria-live="polite">
          <span className={cn("relative inline-flex h-2.5 w-2.5 shrink-0 rounded-full", connectionState === "verbunden" ? "bg-emerald-400" : "bg-amber-400")}><span className={cn("absolute inset-0 animate-ping rounded-full opacity-30 motion-reduce:animate-none", connectionState === "verbunden" ? "bg-emerald-400" : "bg-amber-400")} /></span>
          <span className="min-w-0"><strong className="block text-[9px] uppercase tracking-[0.14em] text-slate-300">Echtzeitverbindung</strong><small className="mt-1 block truncate text-[11px] text-slate-400">v{frontendPackage.version} · SSE · {connectionState}</small></span>
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
            <NotificationCenter targets={targets.data} containers={containers.data ?? []} />
            <AccountMenu session={session} />
            <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary max-[720px]:px-2" type="button" onClick={onLogout}>Abmelden</button>
            <button className="inline-flex h-9 w-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] text-sm text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" type="button" onClick={toggle} aria-label="Theme wechseln" title="Theme wechseln">{theme === "dark" ? "☼" : "☾"}</button>
          </div>
        </header>
        <div className={activityOpen ? "grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(0,1fr)_24rem] items-stretch gap-3 overflow-hidden transition-[grid-template-columns] duration-300 ease-in-out motion-reduce:transition-none max-[1000px]:grid-cols-[minmax(0,1fr)]" : "grid min-h-0 min-w-0 flex-1 grid-cols-1 items-stretch overflow-hidden transition-[grid-template-columns] duration-300 ease-in-out motion-reduce:transition-none"}>
          <section aria-label="Seiteninhalt" className="min-h-0 min-w-0 overflow-x-hidden overflow-y-auto [scrollbar-color:var(--line)_transparent] [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-[var(--line)] [&::-webkit-scrollbar-track]:bg-transparent" tabIndex={-1}>
            <div className="min-h-full px-4 py-3 sm:px-6">
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
                <Route path="/schedules" element={<SchedulesPage isAdmin={session.role === "admin"} />} />
                <Route path="/update-policies" element={<UpdatePoliciesPage />} />
                <Route path="/workflows/:jobId" element={<WorkflowDetailPage />} />
                <Route path="/secrets" element={session.role === "admin" ? <SecretsPage /> : <NotFound />} />
                <Route path="/account/security" element={<AccountPasswordPage />} />
                <Route path="/wiki" element={<WikiPage isAdmin={session.role === "admin"} />} />
                <Route path="/admin/users" element={session.role === "admin" ? <UsersPage /> : <NotFound />} />
                <Route path="/admin/audit" element={session.role === "admin" ? <AuditLogPage /> : <NotFound />} />
                <Route path="/admin/backups" element={session.role === "admin" ? <BackupsPage /> : <NotFound />} />
                <Route path="*" element={<NotFound />} />
              </Routes>
            </div>
          </section>
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
  const { theme, toggle } = useTheme();
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const submit = async (event: React.SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (token.trim().length === 0) {
      setError("Bitte ein API-Token eingeben.");
      return;
    }
    setPending(true);
    setError(null);
    const accepted = await onLogin(token.trim());
    setPending(false);
    if (!accepted) { setToken(""); setError("Token ungültig oder abgelaufen. Bitte erneut versuchen."); }
  };
  return <main className="relative grid min-h-screen place-items-center bg-cover bg-center p-4 text-[var(--ink)]" style={loginBackground(theme)}><LoginThemeToggle theme={theme} onToggle={toggle} /><form className="grid w-full max-w-sm gap-3 border border-[var(--line)] bg-[var(--panel)] p-5 shadow-2xl backdrop-blur-md" noValidate onSubmit={(event) => void submit(event)}><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">lxcup Control Plane</p><h1 className="m-0 text-xl font-semibold">Bei lxcup anmelden</h1><p className="mt-1 text-sm text-[var(--muted)]">Gib das Viewer-, Operator- oder Admin-Token ein.</p></div><label className="grid gap-1 text-xs font-semibold">API-Token<input autoComplete="off" autoFocus className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" type="password" value={token} onChange={(event) => { setToken(event.target.value); setError(null); }} required /></label>{error ? <p className="m-0 text-sm text-[var(--error)]" role="alert">{error}</p> : null}<button className="inline-flex min-h-10 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 text-sm font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={pending}>{pending ? "Prüfe Token …" : "Anmelden"}</button><small className="text-[var(--muted)]">Das Token bleibt nur bis zum Schließen oder Neuladen dieses Tabs im Arbeitsspeicher.</small></form></main>;
}

function AccountLoginScreen({ onLogin }: Readonly<{ onLogin: (username: string, password: string) => Promise<boolean> }>) {
  const { theme, toggle } = useTheme();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const submit = async (event: React.SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (username.trim().length === 0 || password.length === 0) {
      setError("Bitte Benutzername und Passwort eingeben.");
      return;
    }
    setPending(true);
    setError(null);
    const accepted = await onLogin(username.trim(), password);
    setPending(false);
    if (!accepted) { setPassword(""); setError("Benutzername oder Passwort ungültig."); }
  };
  return <main className="relative grid min-h-screen place-items-center bg-cover bg-center p-4 text-[var(--ink)]" style={loginBackground(theme)}><LoginThemeToggle theme={theme} onToggle={toggle} /><form className="grid w-full max-w-sm gap-4 border border-[var(--line)] bg-[var(--panel)] p-6 shadow-2xl backdrop-blur-md" noValidate onSubmit={(event) => void submit(event)}><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">lxcup Control Plane</p><h1 className="m-0 text-xl font-semibold">Anmelden</h1><p className="mt-1 text-sm text-[var(--muted)]">Melde dich mit deinem lokalen Benutzerkonto an.</p></div><label className="grid gap-1.5 text-xs font-semibold">Benutzername<input autoComplete="username" autoFocus className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" value={username} onChange={(event) => { setUsername(event.target.value); setError(null); }} required /></label><label className="grid gap-1.5 text-xs font-semibold">Passwort<PasswordInput autoComplete="current-password" className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" value={password} onChange={(event) => { setPassword(event.target.value); setError(null); }} required /></label>{error ? <p className="m-0 text-sm text-[var(--error)]" role="alert">{error}</p> : null}<button className="inline-flex min-h-10 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 text-sm font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={pending}>{pending ? "Anmeldung läuft …" : "Anmelden"}</button></form></main>;
}

function LoginThemeToggle({ theme, onToggle }: Readonly<{ theme: "dark" | "light"; onToggle: () => void }>) {
  return <button className="absolute right-5 top-5 inline-flex h-10 w-10 items-center justify-center border border-white/20 bg-slate-950/60 text-lg text-white shadow-lg backdrop-blur transition-colors hover:bg-slate-800" type="button" onClick={onToggle} aria-label={theme === "dark" ? "Hellmodus aktivieren" : "Dunkelmodus aktivieren"} title={theme === "dark" ? "Hellmodus aktivieren" : "Dunkelmodus aktivieren"}>{theme === "dark" ? "☼" : "☾"}</button>;
}

function loginBackground(theme: "dark" | "light"): React.CSSProperties {
  const overlay = theme === "dark" ? "linear-gradient(90deg, rgb(7 15 30 / 78%), rgb(7 15 30 / 48%))" : "linear-gradient(90deg, rgb(244 246 250 / 90%), rgb(244 246 250 / 60%))";
  return { backgroundImage: `${overlay}, url('/lxcup-login-background.png')` };
}

function useTheme() {
  const [theme, setTheme] = useState<"dark" | "light">(() => globalThis.localStorage?.getItem("lxcup-theme-v2") === "dark" ? "dark" : "light");
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    globalThis.localStorage?.setItem("lxcup-theme-v2", theme);
  }, [theme]);
  return { theme, toggle: () => setTheme(toggleTheme) };
}

function PasswordOnboardingScreen({ onComplete }: Readonly<{ onComplete: () => void }>) {
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const submit = async (event: React.SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (password.length < 12) { setError("Das Passwort muss mindestens 12 Zeichen enthalten."); return; }
    if (password !== confirmation) { setError("Die Passwörter stimmen nicht überein."); return; }
    setPending(true);
    setError(null);
    try {
      await apiClient.changePassword(password);
      onComplete();
    } catch (error_) {
      setError(error_ instanceof ApiError ? error_.message : "Das Passwort konnte nicht geändert werden.");
    } finally { setPending(false); }
  };
  return <main className="grid min-h-screen place-items-center bg-[var(--paper)] p-4 text-[var(--ink)]"><form className="grid w-full max-w-md gap-4 border border-[var(--line)] bg-[var(--panel)] p-6 shadow-xl" noValidate onSubmit={(event) => void submit(event)}><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Erste Anmeldung</p><h1 className="m-0 text-xl font-semibold">Passwort ändern</h1><p className="mt-1 text-sm text-[var(--muted)]">Lege ein persönliches Passwort fest, bevor du lxcup weiter verwendest. Mindestens 12 Zeichen.</p></div><label className="grid gap-1.5 text-xs font-semibold">Neues Passwort<PasswordInput autoComplete="new-password" autoFocus className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" minLength={12} value={password} onChange={(event) => { setPassword(event.target.value); setError(null); }} required /></label><label className="grid gap-1.5 text-xs font-semibold">Passwort wiederholen<PasswordInput autoComplete="new-password" className="border border-[var(--line)] bg-[var(--paper)] p-2 text-sm" minLength={12} value={confirmation} onChange={(event) => { setConfirmation(event.target.value); setError(null); }} required /></label>{error ? <p className="m-0 text-sm text-[var(--error)]" role="alert">{error}</p> : null}<button className="inline-flex min-h-10 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 text-sm font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={pending}>{pending ? "Wird gespeichert …" : "Passwort speichern"}</button></form></main>;
}

function AuthMessage({ message }: Readonly<{ message: string }>) {
  return <main className="grid min-h-screen place-items-center bg-[var(--paper)] p-4 text-sm text-[var(--ink)]"><output>{message}</output></main>;
}

function AccountMenu({ session }: Readonly<{ session: AuthSession }>) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (event.target instanceof Node && !menuRef.current?.contains(event.target)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  const username = session.username || roleLabel(session.role);
  return <div className="relative" ref={menuRef}>
    <button className="inline-flex min-h-9 max-w-40 items-center gap-2 border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary max-[720px]:max-w-28 max-[720px]:px-2" type="button" aria-haspopup="menu" aria-expanded={open} aria-controls="account-menu" onClick={() => setOpen((value) => !value)}>
      <span className="truncate">{username}</span><span aria-hidden="true" className="text-[10px] text-[var(--muted)]">▾</span>
    </button>
    {open ? <div id="account-menu" className="absolute right-0 top-full z-[70] mt-2 w-64 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)] shadow-xl" role="menu" aria-label="Konto-Menü">
      <div className="mb-3 border-b border-[var(--line)] pb-3">
        <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Angemeldet als</p>
        <strong className="block truncate text-sm">{username}</strong>
        <dl className="mb-0 mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
          <dt className="text-[var(--muted)]">Rolle</dt><dd className="m-0 text-right">{roleLabel(session.role)}</dd>
          <dt className="text-[var(--muted)]">Sitzung</dt><dd className="m-0 text-right">{session.expires_in_seconds === null ? "Ohne Ablauf" : `Läuft in ${formatDuration(session.expires_in_seconds)} ab`}</dd>
        </dl>
      </div>
      <Link className="flex min-h-9 items-center justify-between gap-2 px-2 text-xs font-semibold text-lxcup-primary hover:bg-[var(--primary-soft)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--primary)]" to="/account/security" role="menuitem" onClick={() => setOpen(false)}>
        <span>Passwort ändern</span><span className="text-base leading-none" aria-hidden="true">→</span>
      </Link>
    </div> : null}
  </div>;
}

function roleLabel(role: AuthRole) {
  if (role === "admin") return "Admin";
  if (role === "user") return "User";
  if (role === "operator") return "Operator";
  return "Viewer";
}
function toggleTheme(current: "dark" | "light"): "dark" | "light" { return current === "dark" ? "light" : "dark"; }
function formatDuration(seconds: number) { return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`; }

function WorkerAvailabilityBanner() {
  const availability = useWorkerAvailability();
  if (availability.data?.available === false) return <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1 border border-[#e7c47f] bg-[var(--warning-soft)] px-3 py-2 text-sm text-[var(--warning)]" role="alert"><strong>Ansible-Worker nicht verfügbar</strong><span>Neue Workflows bleiben eingereiht, bis ein Worker wieder aktiv ist.</span><Link className="font-semibold underline" to="/workflows">Workflows ansehen</Link></div>;
  if (availability.data?.artifact_store_available === false) return <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1 border border-[#e7c47f] bg-[var(--warning-soft)] px-3 py-2 text-sm text-[var(--warning)]" role="alert"><strong>Artifact Store nicht verfügbar</strong><span>Der Worker prüft die Verbindung automatisch erneut. Workflows, die Agent-Artefakte benötigen, können bis zur Wiederherstellung fehlschlagen.</span><Link className="font-semibold underline" to="/workflows">Workflows ansehen</Link></div>;
  return null;
}

function NotificationCenter({ targets, containers }: Readonly<{ targets?: TargetDto[]; containers: ContainerDto[] }>) {
  const jobs = useAnsibleJobs();
  const telemetryAlerts = useTelemetryAlerts();
  const workerAvailability = useWorkerAvailability();
  const failed = (jobs.data ?? []).filter((job) => job.status === "failed" || job.status === "reconcile_required");
  const [open, setOpen] = useState(false);
  const [read, setRead] = useState<string[]>(() => readNotificationIds());
  const [alertHistory, setAlertHistory] = useState<TelemetryAlertNotification[]>(() => readTelemetryAlertHistory());
  const [systemAlertHistory, setSystemAlertHistory] = useState<SystemAlertNotification[]>(() => readSystemAlertHistory());
  const alertStatuses = useRef(new Map(alertHistory.map((alert) => [alert.id, alert.status])));
  const systemAlertStatuses = useRef(new Map(systemAlertHistory.map((alert) => [alert.id, alert.status])));
  const notificationCenterRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: MouseEvent) => {
      if (event.target instanceof Node && !notificationCenterRef.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener("click", closeOnOutsideClick);
    return () => document.removeEventListener("click", closeOnOutsideClick);
  }, [open]);
  useEffect(() => {
    if (telemetryAlerts.data === undefined) return;
    const reopenedIds = telemetryAlerts.data.filter((alert) => alertStatuses.current.get(alert.id) === "resolved").map((alert) => alert.id);
    if (reopenedIds.length > 0) {
      const reopened = new Set(reopenedIds);
      setRead((current) => persistReadNotificationIds(current.filter((id) => !reopened.has(id))));
    }
    const currentIds = new Set(telemetryAlerts.data.map((alert) => alert.id));
    const nextStatuses = new Map(alertStatuses.current);
    for (const [id, status] of nextStatuses) {
      if (status === "active" && !currentIds.has(id)) nextStatuses.set(id, "resolved");
    }
    for (const alert of telemetryAlerts.data) nextStatuses.set(alert.id, "active");
    alertStatuses.current = nextStatuses;
    setAlertHistory((previous) => {
      const previousById = new Map(previous.map((alert) => [alert.id, alert]));
      const next = previous.map((alert) => resolveAlertStatus(alert, currentIds));
      for (const alert of telemetryAlerts.data) {
        const old = previousById.get(alert.id);
        const notification: TelemetryAlertNotification = { ...alert, status: "active", ...(old?.resolved_at ? {} : { resolved_at: undefined }) };
        const index = next.findIndex((item) => item.id === alert.id);
        if (index >= 0) next[index] = notification;
        else next.push(notification);
      }
      const sorted = [...next];
      sorted.sort((left, right) => Date.parse(right.observed_at) - Date.parse(left.observed_at));
      const trimmed = sorted.slice(0, 50);
      globalThis.localStorage?.setItem(TELEMETRY_ALERT_HISTORY_STORAGE_KEY, JSON.stringify(trimmed));
      return trimmed;
    });
  }, [telemetryAlerts.data]);
  useEffect(() => {
    const availability = workerAvailability.data;
    if (!availability) return;
    const { active, unknown } = collectSystemAlertSignals(availability, targets, systemAlertStatuses.current);
    const previousStatuses = systemAlertStatuses.current;
    const { nextStatuses, reopened } = transitionSystemAlertStatuses(previousStatuses, active, unknown);
    systemAlertStatuses.current = nextStatuses;
    if (reopened.length) setRead((current) => persistReadNotificationIds(current.filter((id) => !reopened.includes(id))));
    setSystemAlertHistory((previous) => persistSystemAlertHistory(previous, active, unknown));
  }, [workerAvailability.data, targets]);
  const unread = failed.filter((job) => !read.includes(job.id));
  const telemetryUnread = alertHistory.filter((alert) => !read.includes(alert.id));
  const systemUnread = systemAlertHistory.filter((alert) => !read.includes(alert.id));
  const saveRead = (next: string[]) => {
    setRead(persistReadNotificationIds(next));
  };
  const markRead = (id: string) => saveRead(read.includes(id) ? read : [...read, id]);
  const markAllRead = () => saveRead([...new Set([...read, ...failed.map((job) => job.id), ...alertHistory.map((alert) => alert.id), ...systemAlertHistory.map((alert) => alert.id)])]);
  const unreadCount = unread.length + telemetryUnread.length + systemUnread.length;
  return <div className="relative" ref={notificationCenterRef}><button className="relative inline-flex h-9 w-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] text-sm text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" type="button" aria-label="Benachrichtigungen" aria-expanded={open} onClick={() => setOpen((value) => !value)} title="Benachrichtigungen">🔔{unreadCount ? <span className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-600 px-1 text-[10px] text-white">{unreadCount}</span> : null}</button>{open ? <div className="absolute right-0 top-10 z-20 w-[min(24rem,calc(100vw-1rem))] overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)] shadow-xl"><div className="flex items-center justify-between gap-3 border-b border-[var(--line)] px-3 py-2.5"><div><strong className="block">Benachrichtigungen</strong><span className="text-xs text-[var(--muted)]">{failed.length} fehlgeschlagene Workflows · {alertHistory.filter((alert) => alert.status === "active").length} Telemetrie-/Agent-Warnungen · {systemAlertHistory.filter((alert) => alert.status === "active").length} Systemwarnungen</span></div><button className="text-xs font-semibold text-lxcup-primary hover:underline disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={unreadCount === 0} onClick={markAllRead}>Alle als gelesen markieren</button></div>{notificationBody(failed, alertHistory, markRead, targets ?? [], containers, telemetryAlerts.isLoading, systemAlertHistory)}</div> : null}</div>;
}

function readNotificationIds() {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(NOTIFICATION_READ_STORAGE_KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
}

type SystemAlertSignal = Omit<SystemAlertNotification, "status" | "observed_at" | "resolved_at">;

function collectSystemAlertSignals(
  availability: WorkerAvailabilityDto,
  targets: TargetDto[] | undefined,
  previousStatuses: ReadonlyMap<string, SystemAlertNotification["status"]>,
) {
  const active = new Map<string, SystemAlertSignal>();
  const unknown = new Set<string>();
  if (!availability.available) {
    active.set("worker-unavailable", { id: "worker-unavailable", title: "Ansible-Worker nicht verfügbar", message: "Neue Workflows bleiben eingereiht, bis ein Worker wieder aktiv ist.", href: "/workflows" });
    unknown.add("artifact-store-unavailable");
  } else if (availability.artifact_store_available === false) {
    active.set("artifact-store-unavailable", { id: "artifact-store-unavailable", title: "Artifact Store nicht verfügbar", message: "Der Worker prüft die Verbindung automatisch erneut. Workflows, die Agent-Artefakte benötigen, können fehlschlagen.", href: "/workflows" });
  } else if (availability.artifact_store_available == null) {
    unknown.add("artifact-store-unavailable");
  }
  preserveUnknownAgentSignals(previousStatuses, targets, unknown);
  addOutdatedAgentSignals(targets, active);
  return { active, unknown };
}

function preserveUnknownAgentSignals(
  previousStatuses: ReadonlyMap<string, SystemAlertNotification["status"]>,
  targets: TargetDto[] | undefined,
  unknown: Set<string>,
) {
  if (targets !== undefined) return;
  for (const [id, status] of previousStatuses) {
    if (status === "active" && id.startsWith("agent-outdated:")) unknown.add(id);
  }
}

function addOutdatedAgentSignals(targets: TargetDto[] | undefined, active: Map<string, SystemAlertSignal>) {
  for (const target of targets ?? []) {
    if (target.state !== "managed" || !target.agent_version || !target.latest_agent_version || target.agent_version === target.latest_agent_version) continue;
    const id = `agent-outdated:${target.id}`;
    active.set(id, { id, title: `Agent-Version auf ${target.name} veraltet`, message: `Installiert v${target.agent_version}; verfügbar v${target.latest_agent_version}.`, href: `/targets/${target.id}` });
  }
}

function transitionSystemAlertStatuses(
  previous: ReadonlyMap<string, SystemAlertNotification["status"]>,
  active: ReadonlyMap<string, SystemAlertSignal>,
  unknown: ReadonlySet<string>,
) {
  const nextStatuses = new Map(previous);
  const reopened = [...active.keys()].filter((id) => previous.get(id) === "resolved");
  for (const id of active.keys()) nextStatuses.set(id, "active");
  for (const [id, status] of nextStatuses) {
    if (status === "active" && !active.has(id) && !unknown.has(id)) nextStatuses.set(id, "resolved");
  }
  return { nextStatuses, reopened };
}

function persistSystemAlertHistory(
  previous: SystemAlertNotification[],
  active: ReadonlyMap<string, SystemAlertSignal>,
  unknown: ReadonlySet<string>,
) {
  const now = new Date().toISOString();
  const previousById = new Map(previous.map((alert) => [alert.id, alert]));
  const next = previous.map((alert) => updateSystemAlert(alert, active.get(alert.id), unknown, now));
  for (const alert of active.values()) {
    if (!previousById.has(alert.id)) next.push({ ...alert, status: "active", observed_at: now });
  }
  next.sort((left, right) => Date.parse(right.observed_at) - Date.parse(left.observed_at));
  const trimmed = next.slice(0, 20);
  globalThis.localStorage?.setItem(SYSTEM_ALERT_HISTORY_STORAGE_KEY, JSON.stringify(trimmed));
  return trimmed;
}

function updateSystemAlert(
  alert: SystemAlertNotification,
  signal: SystemAlertSignal | undefined,
  unknown: ReadonlySet<string>,
  now: string,
): SystemAlertNotification {
  if (signal) {
    return { ...alert, ...signal, status: "active", observed_at: alert.status === "resolved" ? now : alert.observed_at, resolved_at: undefined };
  }
  if (alert.status === "active" && !unknown.has(alert.id)) return { ...alert, status: "resolved", resolved_at: now };
  return alert;
}

function resolveAlertStatus(alert: TelemetryAlertNotification, currentIds: ReadonlySet<string>): TelemetryAlertNotification {
  if (currentIds.has(alert.id) || alert.status !== "active") return alert;
  return { ...alert, status: "resolved", resolved_at: new Date().toISOString() };
}

function persistReadNotificationIds(ids: string[]) {
  globalThis.localStorage?.setItem(NOTIFICATION_READ_STORAGE_KEY, JSON.stringify(ids));
  return ids;
}

function readTelemetryAlertHistory(): TelemetryAlertNotification[] {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(TELEMETRY_ALERT_HISTORY_STORAGE_KEY) ?? "[]") as TelemetryAlertNotification[];
  } catch {
    return [];
  }
}

function readSystemAlertHistory(): SystemAlertNotification[] {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(SYSTEM_ALERT_HISTORY_STORAGE_KEY) ?? "[]") as SystemAlertNotification[];
  } catch {
    return [];
  }
}

function eventQueryKey(resource: string) {
  if (resource === "target") return queryKeys.targets;
  if (resource === "ansible_job") return queryKeys.ansibleJobs;
  if (resource === "backup") return queryKeys.adminBackups;
  return queryKeys.containers;
}

const exactPageTitles: Readonly<Record<string, string>> = {
  "/": "Übersicht",
  "/servers": "Linux-Server",
  "/containers": "LXC-Container",
  "/docker": "Docker-Container",
  "/windows": "Windows-Systeme",
  "/targets": "Ziele",
  "/enrollments/new": "Ziel einbinden",
  "/workflows": "Tasks & Workflows",
  "/schedules": "Zeitpläne",
  "/update-policies": "Update-Policies",
  "/secrets": "Secrets",
  "/account/security": "Passwort ändern",
  "/admin/users": "Benutzerverwaltung",
  "/admin/audit": "Aktivitätsprotokoll",
  "/wiki": "Wiki",
};

const pageTitleMatchers: readonly { matches: (pathname: string) => boolean; title: string }[] = [
  { matches: (pathname) => pathname.endsWith("/packages"), title: "Paketinventar" },
  { matches: (pathname) => pathname.startsWith("/containers/"), title: "LXC-Details" },
  { matches: (pathname) => pathname.startsWith("/targets/"), title: "Ziel-Details" },
  { matches: (pathname) => pathname.startsWith("/workflows/"), title: "Workflow-Protokoll" },
];

function headerPageTitle(pathname: string) {
  return exactPageTitles[pathname]
    ?? pageTitleMatchers.find(({ matches }) => matches(pathname))?.title
    ?? "Seite nicht gefunden";
}

function notificationBody(failed: AnsibleJobDto[] | undefined, alerts: TelemetryAlertNotification[], markRead: (id: string) => void, targets: TargetDto[], containers: ContainerDto[], loadingAlerts: boolean, systemAlerts: SystemAlertNotification[]) {
  if (failed === undefined || failed.length === 0) {
    const visibleAlerts = alerts.slice(0, 8);
    if (visibleAlerts.length === 0 && systemAlerts.length === 0) return <p className="m-0 px-3 py-4 text-sm text-[var(--muted)]">{loadingAlerts ? "Prüfe Telemetrie…" : "Keine offenen Benachrichtigungen."}</p>;
    return <div className="max-h-[min(32rem,calc(100dvh-5rem))] overflow-y-auto">{systemAlerts.slice(0, 6).map((alert) => <SystemAlertItem key={alert.id} alert={alert} markRead={markRead} />)}{visibleAlerts.map((alert) => <TelemetryAlertItem key={alert.id} alert={alert} markRead={markRead} />)}</div>;
  }
  const visibleAlerts = alerts.slice(0, 8);
  return <div className="max-h-[min(32rem,calc(100dvh-5rem))] overflow-y-auto">
    {systemAlerts.slice(0, 6).map((alert) => <SystemAlertItem key={alert.id} alert={alert} markRead={markRead} />)}
    {visibleAlerts.map((alert) => <TelemetryAlertItem key={alert.id} alert={alert} markRead={markRead} />)}
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
        <span className="justify-self-end text-[10px] font-medium text-[var(--muted)]">{notificationModeLabel(job.mode)} <span className="text-sm leading-none" aria-hidden="true">↗</span></span>
      </Link>;
    })}
  </div>;
}

function SystemAlertItem({ alert, markRead }: Readonly<{ alert: SystemAlertNotification; markRead: (id: string) => void }>) {
  const resolved = alert.status === "resolved";
  return <Link className="grid gap-2 border-b border-[var(--line)] px-3 py-3 text-xs transition-colors last:border-0 hover:bg-[var(--primary-soft)]" to={alert.href ?? "/workflows"} onClick={() => markRead(alert.id)}>
    <div className="flex items-center justify-between gap-2"><span className={cn("inline-flex px-2 py-1 text-[10px] font-bold", resolved ? "bg-[var(--success-soft)] text-[var(--success)]" : "bg-[var(--warning-soft)] text-[var(--warning)]")}>{resolved ? "Behoben" : "Warnung"}</span><time className="text-[10px] text-[var(--muted)]">{new Date(resolved ? alert.resolved_at ?? alert.observed_at : alert.observed_at).toLocaleString()}</time></div>
    <strong className="text-sm">{resolved ? `${alert.title} · behoben` : alert.title}</strong>
    <span className="text-xs text-[var(--muted)]">{resolved ? "Die Ursache ist nicht mehr aktiv." : alert.message}</span>
    <span className="text-[10px] font-medium text-lxcup-primary">{alert.href?.startsWith("/targets/") ? "Ressource öffnen ↗" : "Workflows öffnen ↗"}</span>
  </Link>;
}

function TelemetryAlertItem({ alert, markRead }: Readonly<{ alert: TelemetryAlertNotification; markRead: (id: string) => void }>) {
  const resolved = alert.status === "resolved";
  const severityLabel = telemetrySeverityLabel(resolved, alert.severity);
  const metricValue = telemetryMetricValue(alert);
  const statusClass = telemetryAlertStatusClass(alert, resolved);
  const recoveryMessage = telemetryRecoveryMessage(alert);
  return <Link className="grid gap-2 border-b border-[var(--line)] px-3 py-3 text-xs transition-colors last:border-0 hover:bg-[var(--primary-soft)]" to={`/targets/${alert.target_id}`} onClick={() => markRead(alert.id)}>
    <div className="flex items-center justify-between gap-2"><span className={cn("inline-flex px-2 py-1 text-[10px] font-bold", statusClass)}>{severityLabel}</span><time className="text-[10px] text-[var(--muted)]">{new Date(resolved ? alert.resolved_at ?? alert.observed_at : alert.triggered_at).toLocaleString()}</time></div>
    <strong className="text-sm">{telemetryMetricTitle(alert.metric)}</strong>
    <span className="grid min-w-0 gap-0.5 border-l-2 border-l-lxcup-primary pl-2.5"><span className="text-[10px] font-bold uppercase tracking-wide text-[var(--muted)]">Ressource</span><strong className="truncate text-sm font-semibold">{alert.target_name}</strong><span className="truncate text-xs text-[var(--muted)]">{alert.target_kind} · {alert.target_address}</span></span>
    <span className="text-xs text-[var(--muted)]">{resolved ? recoveryMessage : metricValue}</span>
  </Link>;
}

function telemetryMetricTitle(metric: string) {
  if (metric === "Agent-Verbindung") return "Agent nicht verbunden";
  if (metric === "Telemetrie") return "Telemetrie veraltet";
  return `Hohe ${metric}-Auslastung`;
}

function telemetrySeverityLabel(resolved: boolean, severity: TelemetryAlertNotification["severity"]) {
  if (resolved) return "Behoben";
  return severity === "critical" ? "Kritisch" : "Warnung";
}

function telemetryAlertStatusClass(alert: TelemetryAlertNotification, resolved: boolean) {
  if (resolved) return "bg-[var(--success-soft)] text-[var(--success)]";
  return alert.severity === "critical" ? "bg-[var(--error-soft)] text-[var(--error)]" : "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function telemetryMetricValue(alert: TelemetryAlertNotification) {
  if (alert.metric === "Agent-Verbindung") return `Letzter Heartbeat vor ${formatAlertAge(alert.age_seconds ?? 0)}`;
  if (alert.metric === "Telemetrie") return `Letzter Messpunkt vor ${formatAlertAge(alert.age_seconds ?? 0)}`;
  return `${formatAlertPercent(alert.value_basis_points)} · Grenzwert ${formatAlertPercent(alert.threshold_basis_points)}`;
}

function telemetryRecoveryMessage(alert: TelemetryAlertNotification) {
  if (alert.metric === "Agent-Verbindung") return "Agent-Heartbeat ist wieder aktuell.";
  return alert.metric === "Telemetrie" ? "Telemetrie wieder aktuell." : "Auslastung wieder im Normalbereich.";
}

function formatAlertPercent(value: number | null) {
  return value === null ? "—" : `${(value / 100).toFixed(1)}%`;
}

function formatAlertAge(seconds: number) {
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} Min.`;
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
  const location = useLocation();
  return <section className="grid min-h-[55vh] place-items-center border border-[var(--line)] bg-[var(--panel)] p-6 text-center text-[var(--ink)]"><div className="grid justify-items-center gap-3"><span className="text-6xl font-extrabold tracking-tight text-lxcup-primary" aria-hidden="true">404</span><div><h1 className="mb-1 text-xl font-semibold">Diese Seite gibt es nicht</h1><p className="mb-0 text-sm text-[var(--muted)]">Der aufgerufene Pfad <code>{location.pathname}</code> wurde nicht gefunden.</p></div><Link className="inline-flex min-h-9 items-center border border-lxcup-primary bg-lxcup-primary px-4 text-xs font-semibold text-white hover:bg-blue-700" to="/">Zur Übersicht</Link></div></section>;
}

function NavigationSection({ title, children }: Readonly<{ title: string; children: React.ReactNode }>) {
  return <section className="grid gap-1" aria-label={title}>
    <h2 className="mb-1 px-2 text-[9px] font-bold uppercase tracking-[0.16em] text-slate-500 max-[720px]:hidden">{title}</h2>
    {children}
  </section>;
}

type NavigationIconName = "overview" | "server" | "lxc" | "docker" | "windows" | "workflows" | "schedules" | "policies" | "secrets" | "users" | "audit" | "backup" | "help";

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
    users: <><circle cx="8" cy="7" r="3" /><path d="M2.5 18v-1a5.5 5.5 0 0 1 11 0v1M15 5a3 3 0 0 1 0 5.8M16 13a5 5 0 0 1 3.5 4.8V18" /></>,
    audit: <><path d="M4 3h12v15H4zM7 7h6M7 10h6M7 13h4" /><path d="M2 6v14h12" /></>,
    backup: <><path d="M4 3h9l3 3v11H4z" /><path d="M7 3v5h6V3M7 17v-5h6v5" /><path d="M10 9v3m-1.5-1.5L10 9l1.5 1.5" /></>,
    help: <><circle cx="10" cy="10" r="7" /><path d="M7.8 7.5a2.3 2.3 0 0 1 4.4.9c0 1.6-2.2 2-2.2 3.6M10 15.3h.01" /></>,
  };
  return <svg className={cn("h-[17px] w-[17px] shrink-0 transition-colors", active ? "text-lxcup-primary" : "text-slate-400 group-hover:text-slate-200")} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{shapes[name]}</svg>;
}

function NavigationLink({ to, label, icon }: Readonly<{ to: string; label: string; icon: NavigationIconName }>) {
  return <NavLink aria-label={label} title={label} className={({ isActive }) => cn("group flex min-h-9 items-center gap-3 border border-transparent border-l-2 px-3 py-2 text-[12px] font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-blue-400", isActive ? "border-[var(--line)] border-l-[var(--primary)] bg-[var(--nav-surface)] text-white" : "text-slate-300 hover:border-slate-700 hover:bg-[var(--nav-surface)] hover:text-white")} to={to}>{({ isActive }) => <><NavigationIcon name={icon} active={isActive} /><span className="truncate">{label}</span></>}</NavLink>;
}

export { DataTable } from "./components/DataTable";
