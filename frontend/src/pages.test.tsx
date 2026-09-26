import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, type ContainerDto, type DockerWorkloadDto, type SecretMetadata, type TargetDto, type WorkerAvailabilityDto } from "./api";

const mocks = vi.hoisted(() => ({
  targets: { data: [] as TargetDto[], isLoading: false, error: null as Error | null },
  containers: { data: [] as ContainerDto[], isLoading: false, error: null as Error | null },
  jobs: { data: [] as any[], isLoading: false, error: null as Error | null },
  job: { data: undefined as any, isLoading: false, error: null as Error | null },
  events: { data: [], isLoading: false, error: null as Error | null },
  enrollment: { data: undefined as any, isLoading: false, error: null as Error | null },
  workloads: { data: [] as DockerWorkloadDto[], isLoading: false, error: null as Error | null },
  dockerDiscovery: { data: null as any, isLoading: false, error: null as Error | null },
  workerAvailability: { data: { available: true, last_seen_at: "2026-01-01T00:00:00Z" } as WorkerAvailabilityDto, isLoading: false, error: null as Error | null },
  packageInventory: { data: undefined as any, isLoading: false, error: null as Error | null },
  telemetry: { data: undefined as any, isLoading: false, error: null as Error | null },
  telemetryAlerts: { data: [] as any[], isLoading: false, error: null as Error | null },
  schedules: { data: [] as any[], isLoading: false, error: null as Error | null },
  updatePolicies: { data: [] as any[], isLoading: false, error: null as Error | null },
  listSecrets: vi.fn(async () => [] as SecretMetadata[]),
  listSecretAudit: vi.fn(async () => []),
  createSecret: vi.fn(async (request: any) => ({ metadata: { metadata: { id: "secret-new", name: request.name, kind: request.kind, scope: request.scope, created_at: "2026-01-01", updated_at: "2026-01-01" }, status: "active" } })),
  createTarget: vi.fn(async () => ({ ...target, state: "pending" as const })),
  createAnsibleJob: vi.fn(async (request: any) => ({ id: request.operation === "health_check" ? "job-health" : "job-deploy", operation: request.operation, playbook: "agent/deploy.yml", playbook_version: "v1", target: { target: "target-1" }, mode: request.mode, status: "queued", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" })),
  retryAnsibleJob: vi.fn(async (id: string) => ({ id, operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { target: "target-1" }, mode: "check", status: "queued", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" })),
  createEnrollment: vi.fn(async () => ({ id: "enrollment-1" })),
  createContainerAction: vi.fn(async () => ({ id: "task-1", status: "queued" })),
  createSchedule: vi.fn(async (request: any) => ({ ...request, last_run_at: null, next_run_at: "2026-01-01T01:00:00Z", last_error: null })),
  createUpdatePolicy: vi.fn(async (request: any) => request),
  deleteUpdatePolicy: vi.fn(async () => undefined),
  getAgentHealth: vi.fn(async () => ({ healthy: true, info: { agent_id: "agent-1", platform: "linux", hostname: "host", version: "0.3.1", protocol_version: "1" }, metrics: { collected_at: "2026-01-01", commands_total: 2, commands_failed: 0, last_command_at: null } })),
  getAgentMetrics: vi.fn(async () => ({ commands_total: 2, commands_failed: 0, last_command_at: null })),
  discoverDockerWorkloads: vi.fn(async () => undefined),
  discoverTargetDocker: vi.fn(async () => ({ target_id: "target-1", target_name: "test-target", available: true, reason: null, collected_at: "2026-01-01T00:00:00Z", containers: [{ id: "docker-target-1", name: "web", image: "nginx:1", state: "running", status: "Up", ports: ["80/tcp"], started_at: null, labels: [] }] })),
  adoptDockerWorkload: vi.fn(async () => undefined),
  removeDockerWorkload: vi.fn(async () => undefined),
  subscribe: vi.fn(() => () => undefined),
  setCredentials: vi.fn(),
  setUnauthorizedHandler: vi.fn(),
  getSession: vi.fn(async () => ({ role: "admin" as const, expires_in_seconds: null })),
  logout: vi.fn(async () => undefined),
}));

const userEvent = {
  click: async (element: Element) => { fireEvent.click(element); },
  type: async (element: Element, value: string) => { const input = element as HTMLInputElement; fireEvent.change(input, { target: { value: `${input.value}${value}` } }); },
  selectOptions: async (element: Element, value: string) => { fireEvent.change(element, { target: { value } }); },
};

const target: TargetDto = { id: "target-1", name: "test-target", kind: "lxc", address: "192.0.2.10", transport: "ssh", ssh_user: "lxcup", credential_secret_ref: "cred", ssh_known_hosts_secret_ref: "known", agent_secret_ref: "agent", state: "managed", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
const container: ContainerDto = { id: 101, node_id: "node-1", name: "web-lxc", operating_system: "Debian", status: "running", management_state: "managed", discovered_at: "2026-01-01T00:00:00Z" };
const secret = (id: string, name: string, kind: "ssh_password" | "ssh_known_hosts" | "agent_token" = "ssh_password") => ({ metadata: { metadata: { id, name, kind, scope: { type: "global" as const }, created_at: "2026-01-01", updated_at: "2026-01-01" }, status: "active" as const } });

vi.mock("./queries", () => ({
  queryKeys: { targets: ["targets"], nodes: ["nodes"], ansibleJobs: ["ansible-jobs"], containers: ["containers"], dockerWorkloads: (id: number) => ["docker", id], dockerDiscovery: (id: number) => ["docker-discovery", id], packageInventory: (id: string) => ["targets", id, "package-inventory"], telemetry: (id: string) => ["targets", id, "telemetry"], telemetryAlerts: ["telemetry-alerts"], schedules: ["schedules"], updatePolicies: ["update-policies"] },
  useTargets: () => mocks.targets,
  useContainers: () => mocks.containers,
  useAnsibleJobs: () => mocks.jobs,
  useAnsibleJob: () => mocks.job,
  useAnsibleJobEvents: () => mocks.events,
  useEnrollment: () => mocks.enrollment,
  useDockerWorkloads: () => mocks.workloads,
  useDockerDiscovery: () => mocks.dockerDiscovery,
  useWorkerAvailability: () => mocks.workerAvailability,
  usePackageInventory: () => mocks.packageInventory,
  useTargetTelemetry: () => mocks.telemetry,
  useTelemetryAlerts: () => mocks.telemetryAlerts,
  useSchedules: () => mocks.schedules,
  useUpdatePolicies: () => mocks.updatePolicies,
}));

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return { ...actual, listSecrets: mocks.listSecrets, listSecretAudit: mocks.listSecretAudit, createSecret: mocks.createSecret, createTarget: mocks.createTarget, createAnsibleJob: mocks.createAnsibleJob, retryAnsibleJob: mocks.retryAnsibleJob, createEnrollment: mocks.createEnrollment, createContainerAction: mocks.createContainerAction, createSchedule: mocks.createSchedule, createUpdatePolicy: mocks.createUpdatePolicy, deleteUpdatePolicy: mocks.deleteUpdatePolicy, getAgentHealth: mocks.getAgentHealth, getAgentMetrics: mocks.getAgentMetrics, discoverDockerWorkloads: mocks.discoverDockerWorkloads, discoverTargetDocker: mocks.discoverTargetDocker, adoptDockerWorkload: mocks.adoptDockerWorkload, removeDockerWorkload: mocks.removeDockerWorkload, apiClient: { subscribe: mocks.subscribe, setCredentials: mocks.setCredentials, setUnauthorizedHandler: mocks.setUnauthorizedHandler, getSession: mocks.getSession, logout: mocks.logout } };
});

import { Dashboard } from "./pages/Dashboard";
import { ContainerDetailPage } from "./pages/ContainerDetailPage";
import { DockerPage } from "./pages/DockerPage";
import { SecretsPage } from "./pages/SecretsPage";
import { TargetsPage } from "./pages/TargetsPage";
import { EnrollmentPage } from "./pages/EnrollmentPage";
import { WorkflowsPage } from "./pages/WorkflowsPage";
import { WorkflowDetailPage } from "./pages/WorkflowDetailPage";
import App from "./App";
import { ResourceTree } from "./components/ResourceTree";
import { TargetDetailPage } from "./pages/TargetDetailPage";
import { PackageInventoryPage } from "./pages/PackageInventoryPage";
import { SchedulesPage } from "./pages/SchedulesPage";
import { UpdatePoliciesPage } from "./pages/UpdatePoliciesPage";
import { ContainersPage } from "./pages/ContainersPage";

function renderPage(element: React.ReactElement, route = "/") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}><Routes><Route path="/containers/:containerId/*" element={element} /><Route path="/workflows/:jobId/*" element={element} /><Route path="/targets/:targetId/*" element={element} /><Route path="*" element={element} /></Routes></MemoryRouter></QueryClientProvider>);
}

function TargetAreaSwitcher() {
  const [area, setArea] = useState<"linux_server" | "lxc">("linux_server");
  return <><button type="button" onClick={() => setArea("linux_server")}>Serverbereich</button><button type="button" onClick={() => setArea("lxc")}>LXC-Bereich</button><TargetsPage area={area} /></>;
}

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

beforeEach(() => {
  mocks.targets.data = [];
  mocks.targets.isLoading = false;
  mocks.targets.error = null;
  mocks.containers.data = [];
  mocks.containers.isLoading = false;
  mocks.containers.error = null;
  mocks.jobs.data = [];
  mocks.jobs.isLoading = false;
  mocks.jobs.error = null;
  mocks.job.data = undefined;
  mocks.job.isLoading = false;
  mocks.job.error = null;
  mocks.events.data = [];
  mocks.events.isLoading = false;
  mocks.events.error = null;
  mocks.enrollment.data = undefined;
  mocks.enrollment.isLoading = false;
  mocks.enrollment.error = null;
  mocks.workloads.data = [];
  mocks.dockerDiscovery.data = null;
  mocks.workerAvailability.data = { available: true, last_seen_at: "2026-01-01T00:00:00Z" };
  mocks.workloads.isLoading = false;
  mocks.workloads.error = null;
  mocks.packageInventory.data = undefined;
  mocks.packageInventory.isLoading = false;
  mocks.packageInventory.error = null;
  mocks.telemetry.data = undefined;
  mocks.telemetry.isLoading = false;
  mocks.telemetry.error = null;
  mocks.telemetryAlerts.data = [];
  mocks.telemetryAlerts.isLoading = false;
  mocks.telemetryAlerts.error = null;
  mocks.schedules.data = [];
  mocks.schedules.isLoading = false;
  mocks.schedules.error = null;
  mocks.updatePolicies.data = [];
  mocks.updatePolicies.isLoading = false;
  mocks.updatePolicies.error = null;
  mocks.listSecrets.mockResolvedValue([secret("cred", "ssh-password"), secret("known", "known-hosts", "ssh_known_hosts"), secret("agent", "agent-token", "agent_token")] as any);
  mocks.listSecretAudit.mockResolvedValue([{ secret_id: "cred", action: "created", role: "admin", occurred_at: "2026-01-01T00:00:00Z" }] as any);
  vi.clearAllMocks();
  Object.defineProperty(window, "confirm", { configurable: true, value: vi.fn(() => true) });
});

describe("inventory pages", () => {
  it("renders resource tree entries and empty state", () => {
    mocks.targets.data = [target];
    renderPage(<ResourceTree />);
    expect(screen.getAllByText("test-target").length).toBeGreaterThan(0);
    cleanup();
    mocks.targets.data = [{ ...target, state: "pending" }];
    renderPage(<ResourceTree />);
    expect(screen.getByText("test-target")).toBeInTheDocument();
    cleanup();
    mocks.targets.data = [];
    renderPage(<ResourceTree />);
    expect(screen.getByText("Keine Zugänge angelegt")).toBeInTheDocument();
    cleanup();
    mocks.targets.isLoading = true;
    renderPage(<ResourceTree />);
    expect(screen.queryByText("Keine Zugänge angelegt")).not.toBeInTheDocument();
  });

  it("renders the dashboard system overview, resources, and recent workflows", () => {
    mocks.targets.data = [target];
    mocks.jobs.data = [{ id: "dashboard-job", operation: "health_check", target: { target: target.id }, status: "failed", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:01:00Z" }];
    renderPage(<Dashboard />);
    expect(screen.getByRole("heading", { name: "Übersicht" })).toBeInTheDocument();
    expect(screen.getByText("test-target")).toBeInTheDocument();
    expect(screen.getByText("Verbunden")).toBeInTheDocument();
    expect(screen.getByText("Worker · Verfügbar")).toBeInTheDocument();
    expect(screen.getByText("Healthcheck")).toBeInTheDocument();
    expect(screen.getByText("Fehlgeschlagen")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /LXC-Container.*1 Ziele.*Manuell eingebundene LXCs/ })).toHaveAttribute("href", "/containers");
  });

  it("shows resource onboarding shortcuts on an empty dashboard", () => {
    renderPage(<Dashboard />);
    expect(screen.getByText("Es sind noch keine Ressourcen registriert. Wähle unten den passenden Ressourcenbereich aus.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Linux-Server/ })).toHaveAttribute("href", "/servers");
    expect(screen.queryByRole("link", { name: /LXC einbinden/ })).not.toBeInTheDocument();
  });

  it("renders missing and healthy container details and submits an action", async () => {
    const missing = renderPage(<ContainerDetailPage />, "/containers/999");
    expect(missing.getByText("LXC nicht gefunden")).toBeInTheDocument();
    missing.unmount();
    mocks.containers.data = [container];
    renderPage(<ContainerDetailPage />, "/containers/101");
    expect(await screen.findByText("Healthy")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.createContainerAction).toHaveBeenCalledWith(101, "refresh", true);
  });

  it("renders target detail with inventory, telemetry and stable Docker host mapping", async () => {
    mocks.targets.data = [target];
    mocks.containers.data = [{ ...container, name: "different-display-name", target_id: target.id }];
    mocks.workloads.data = [{ host_container_id: 101, id: "docker-1", name: "web", image: "nginx:latest", state: "running", status: "Up", ports: [], started_at: null, labels: [], presence: "present", change_state: "unchanged", management_state: "managed", discovered_at: "2026-01-01T00:00:00Z" }];
    mocks.jobs.data = [{ id: "job-1", operation: "health_check", target: { target: "target-1" }, status: "succeeded", updated_at: "2026-01-01T00:00:00Z" }];
    mocks.packageInventory.data = { target_id: "target-1", status: "complete", collected_at: "2026-01-01T00:00:00Z", packages: [{ name: "curl", installed_version: "8.5", architecture: "amd64", source: "apt" }] };
    mocks.telemetry.data = { target_id: "target-1", collected_at: "2026-01-01T00:00:00Z", samples: [{ collected_at: "2026-01-01T00:00:00Z", cpu_basis_points: 2500, memory_basis_points: 5000, storage_basis_points: 7500, load_1_milli: 1000, network_rx_bytes: null, network_tx_bytes: null, process_count: null }] };
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByRole("heading", { name: "test-target" })).toBeInTheDocument();
    expect(screen.getByText("Veraltet")).toBeInTheDocument();
    expect(screen.getByText(/Der letzte Heartbeat liegt mehr als 2 Minuten zurück/)).toBeInTheDocument();
    expect(screen.getByText(/1 Pakete/)).toBeInTheDocument();
    expect(screen.getByText("Healthcheck")).toBeInTheDocument();
    const workflowLink = screen.getByRole("link", { name: /Healthcheck/ });
    expect(workflowLink).toHaveAttribute("href", "/workflows/job-1");
    expect(workflowLink.querySelector("svg")).toHaveClass("h-5", "w-5");
    expect(workflowLink.querySelector("small")).toHaveAttribute("title", "Job job-1");
    expect(screen.getByRole("figure", { name: /CPU-Auslastung im Verlauf/ })).toBeInTheDocument();
    expect(screen.getAllByRole("figure", { name: /letzten 10 Minuten/ })).toHaveLength(3);
    expect(screen.getByRole("figure", { name: /RAM-Auslastung im Verlauf/ })).toBeInTheDocument();
    expect(screen.getByRole("figure", { name: /Speicher-Auslastung im Verlauf/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Docker-Inventar öffnen/ })).toHaveAttribute("href", "/docker?host=101");
    await userEvent.click(screen.getByRole("button", { name: "Docker erkennen" }));
    expect(mocks.discoverDockerWorkloads).toHaveBeenCalledWith(101);
    expect(screen.getByRole("link", { name: /Alle Workflows/ })).toHaveAttribute("href", "/workflows?target=target-1");
    expect(screen.getByText("web")).toBeInTheDocument();
  });

  it("plots telemetry against the rolling time window and gives telemetry the full row", () => {
    vi.useFakeTimers();
    try {
      const now = new Date("2026-09-26T10:00:30Z");
      vi.setSystemTime(now);
      mocks.targets.data = [{ ...target, updated_at: now.toISOString() }];
      mocks.containers.data = [{ ...container, target_id: target.id }];
      mocks.telemetry.data = {
        target_id: target.id,
        collected_at: now.toISOString(),
        partial: true,
        missing_samples: 2,
        samples: [-20, -10, 0].map((seconds, index) => ({
          collected_at: new Date(now.getTime() + seconds * 1_000).toISOString(),
          cpu_basis_points: 1_000 + index * 1_000,
          memory_basis_points: 2_000 + index * 1_000,
          storage_basis_points: 3_000 + index * 1_000,
          load_1_milli: 0,
          network_rx_bytes: null,
          network_tx_bytes: null,
          process_count: null,
        })),
      };

      renderPage(<TargetDetailPage />, "/targets/target-1");

      const telemetryFigure = screen.getByRole("figure", { name: /CPU-Auslastung im Verlauf/ });
      const plottedX = [...telemetryFigure.querySelectorAll("circle")].map((point) => Math.round(Number(point.getAttribute("cx"))));
      expect(plottedX).toEqual([571, 580, 590]);
      expect(screen.getByRole("status")).toHaveTextContent("fehlen 2 erwartete Messpunkte");
      expect(telemetryFigure.querySelector('line[stroke="var(--primary)"]')).toBeInTheDocument();

      const telemetryCard = screen.getByRole("heading", { name: "Systemauslastung" }).closest("article");
      const dockerCard = screen.getByRole("heading", { name: "Docker-Inventar" }).closest("article");
      expect(telemetryCard).toHaveClass("xl:col-span-2");
      expect(dockerCard).not.toHaveClass("xl:col-span-2");
      expect(dockerCard && telemetryCard && (dockerCard.compareDocumentPosition(telemetryCard) & Node.DOCUMENT_POSITION_FOLLOWING)).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not infer a Docker host from matching display names", () => {
    mocks.targets.data = [target];
    mocks.containers.data = [{ ...container, name: target.name }];
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText(/Keine alte Container-Verknüpfung nötig/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Docker-Erkennung öffnen/ })).toHaveAttribute("href", "/docker?target=target-1");
    expect(screen.queryByRole("button", { name: "Docker erkennen" })).not.toBeInTheDocument();
  });

  it("shows Docker discovery status and actions in the LXC inventory", async () => {
    mocks.containers.data = [container];
    mocks.dockerDiscovery.data = { host_container_id: 101, status: "succeeded", started_at: "2026-01-01T00:00:00Z", finished_at: "2026-01-01T00:00:02Z", container_count: 2, error_code: null };
    renderPage(<ContainersPage />, "/containers");
    expect(screen.getByText("2 Container")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Öffnen" })).toHaveAttribute("href", "/docker?host=101");
    await userEvent.click(screen.getByRole("button", { name: "Erkennen" }));
    expect(mocks.discoverDockerWorkloads).toHaveBeenCalledWith(101);
  });

  it("updates the heartbeat warning when the two-minute limit passes without reloading", async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
      mocks.targets.data = [{ ...target, updated_at: "2026-01-01T00:00:00Z" }];
      renderPage(<TargetDetailPage />, "/targets/target-1");

      expect(screen.queryByText(/Der letzte Heartbeat liegt mehr als 2 Minuten zurück/)).not.toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(120_001); });
      vi.setSystemTime(new Date("2026-01-01T00:02:00.001Z"));
      await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
      expect(screen.getByText(/Der letzte Heartbeat liegt mehr als 2 Minuten zurück/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("covers target detail loading, missing, empty and dependency errors", () => {
    mocks.targets.isLoading = true;
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("Ziel wird geladen…")).toBeInTheDocument();
    cleanup();
    mocks.targets.isLoading = false;
    mocks.targets.data = [];
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("Ziel nicht gefunden.")).toBeInTheDocument();
    cleanup();
    mocks.targets.data = [{ ...target, state: "pending" }];
    mocks.packageInventory.error = new Error("inventory failure");
    mocks.telemetry.error = new Error("telemetry failure");
    mocks.jobs.data = [{ id: "job-node", operation: "health_check", target: { node: "node-1" }, status: "failed", updated_at: "2026-01-01" }];
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("inventory failure")).toBeInTheDocument();
    expect(screen.getByText("telemetry failure")).toBeInTheDocument();
    expect(screen.getByText("Keine Workflows vorhanden.")).toBeInTheDocument();
  });

  it("shows Docker inventory loading, empty, and error states on a target", () => {
    mocks.targets.data = [target];
    mocks.containers.data = [{ ...container, name: target.name, target_id: target.id }];
    mocks.workloads.isLoading = true;
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("Docker-Inventar wird geladen…")).toBeInTheDocument();
    cleanup();
    mocks.workloads.isLoading = false;
    mocks.workloads.data = [];
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("Noch kein Docker-Inventar für diesen Host.")).toBeInTheDocument();
    cleanup();
    mocks.workloads.error = new Error("docker inventory unavailable");
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("docker inventory unavailable")).toBeInTheDocument();
  });

  it("renders windows and disabled target branches with a failed workflow", () => {
    mocks.targets.data = [{ ...target, kind: "windows_server", transport: "winrm", state: "disabled", agent_version: null, updated_at: new Date().toISOString() }];
    mocks.jobs.data = [{ id: "job-failed", operation: "deploy_agent", target: { target: "target-1" }, status: "failed", updated_at: "2026-01-01" }];
    renderPage(<TargetDetailPage />, "/targets/target-1");
    expect(screen.getByText("Deaktiviert")).toBeInTheDocument();
    expect(screen.getByText("Windows")).toBeInTheDocument();
    expect(screen.getByText("Noch nicht erhoben")).toBeInTheDocument();
    expect(screen.getByText("Agent installieren")).toBeInTheDocument();
  });

  it("filters workflow detail links to the selected target", () => {
    mocks.targets.data = [target];
    mocks.jobs.data = [
      { id: "job-target", operation: "health_check", target: { target: "target-1" }, status: "succeeded", created_at: "2026-01-01", updated_at: "2026-01-01" },
      { id: "job-other", operation: "deploy_agent", target: { target: "target-2" }, status: "failed", created_at: "2026-01-01", updated_at: "2026-01-01" },
    ];
    renderPage(<WorkflowsPage />, "/workflows?target=target-1");
    expect(screen.getByRole("link", { name: /Healthcheck/ })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Agent installieren/ })).not.toBeInTheDocument();
    expect(screen.getByText("Ausführungen für test-target.")).toBeInTheDocument();
  });

  it("searches and sorts package inventory without mixing in system telemetry", () => {
    mocks.targets.data = [target];
    mocks.jobs.data = [{ id: "inventory-job", operation: "collect_package_inventory", target: { target: "target-1" }, status: "succeeded", created_at: "2026-01-01T00:01:00Z" }];
    mocks.packageInventory.data = { target_id: "target-1", status: "complete", collected_at: "2026-01-01T00:00:00Z", packages: [
      { name: "curl", installed_version: "8.5", candidate_version: "8.6", architecture: "amd64", source: "apt" },
      { name: "zlib", installed_version: "1.2", candidate_version: null, architecture: null, source: null },
    ] };
    mocks.telemetry.data = { target_id: "target-1", collected_at: "2026-01-01T00:00:00Z", samples: [
      { collected_at: "2026-01-01T00:00:00Z", cpu_basis_points: 1200, memory_basis_points: 3400, storage_basis_points: 5600, load_1_milli: 100, network_rx_bytes: null, network_tx_bytes: null, process_count: null },
    ] };
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    expect(screen.queryByRole("heading", { name: "Systemauslastung" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Inventarisierungs-Workflow · Erfolgreich" })).toHaveAttribute("href", "/workflows/inventory-job");
    expect(screen.getByText("2 installierte Pakete")).toBeInTheDocument();
    expect(screen.getByText("8.6 · Update verfügbar")).toBeInTheDocument();
    expect(screen.getByText("Noch nicht geprüft")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("z. B. curl oder 8.5"), { target: { value: "zlib" } });
    expect(screen.getByText("zlib")).toBeInTheDocument();
    expect(screen.queryByText("curl")).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Sortierung"), { target: { value: "version" } });
  });

  it("keeps 30-second telemetry current and updates stale status after two minutes", async () => {
    vi.useFakeTimers();
    try {
      const now = new Date("2026-09-25T12:00:00Z");
      vi.setSystemTime(now);
      mocks.targets.data = [target];
      mocks.telemetry.data = {
        target_id: target.id,
        collected_at: new Date(now.getTime() - 30_000).toISOString(),
        samples: [{ collected_at: new Date(now.getTime() - 30_000).toISOString(), cpu_basis_points: 1000, memory_basis_points: 2000, storage_basis_points: 3000, load_1_milli: 0, network_rx_bytes: null, network_tx_bytes: null, process_count: null }],
      };
      mocks.targets.data = [{ ...target, updated_at: now.toISOString() }];
      renderPage(<TargetDetailPage />, "/targets/target-1");
      expect(screen.getByText("Letzter Messpunkt vor 30 s.")).toBeInTheDocument();
      expect(screen.queryByText(/Messwerte sind seit mehr als 2 Minuten veraltet/)).not.toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(90_000); });
      expect(screen.queryByText(/Messwerte sind seit mehr als 2 Minuten veraltet/)).not.toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(5_001); });
      expect(screen.getByText(/Messwerte sind seit mehr als 2 Minuten veraltet/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("starts one package inventory job and prevents duplicate runs while it is queued", async () => {
    mocks.targets.data = [target];
    mocks.packageInventory.data = { target_id: target.id, status: "not_collected", collected_at: null, packages: [] };
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");

    const startButton = screen.getByRole("button", { name: "Inventarisierung starten" });
    await userEvent.click(startButton);

    await waitFor(() => expect(mocks.createAnsibleJob).toHaveBeenCalledWith(expect.objectContaining({
      operation: "collect_package_inventory",
      target_id: target.id,
      mode: "check",
      parameters: { operation: "collect_package_inventory" },
      confirmed: true,
    })));
    expect(screen.getByRole("link", { name: "Inventarisierungs-Workflow · Wartet" })).toHaveAttribute("href", "/workflows/job-deploy");
    expect(screen.getByRole("button", { name: "Inventarisierung läuft" })).toBeDisabled();
  });

  it("shows job submission failures and blocks inventory on an unconnected target", async () => {
    mocks.targets.data = [{ ...target, state: "pending" }];
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    expect(screen.getByRole("button", { name: "Inventarisierung starten" })).toBeDisabled();
    expect(screen.getByText("Die Erfassung ist erst möglich, wenn das Ziel verbunden ist.")).toBeInTheDocument();

    cleanup();
    mocks.targets.data = [target];
    mocks.createAnsibleJob.mockRejectedValueOnce(new Error("worker unavailable"));
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    await userEvent.click(screen.getByRole("button", { name: "Inventarisierung starten" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Inventarisierung konnte nicht gestartet werden: worker unavailable");
  });

  it("shows inventory loading, empty and error states without telemetry", () => {
    mocks.targets.data = [target];
    mocks.packageInventory.isLoading = true;
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    expect(screen.getByText("Paketinventar wird geladen…")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Systemauslastung" })).not.toBeInTheDocument();
    cleanup();
    mocks.packageInventory.isLoading = false;
    mocks.packageInventory.data = { target_id: "target-1", status: "not_collected", collected_at: null, packages: [] };
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    expect(screen.getByText("Noch kein Paketinventar")).toBeInTheDocument();
    cleanup();
    mocks.packageInventory.data = undefined;
    mocks.packageInventory.error = new Error("inventory error");
    renderPage(<PackageInventoryPage />, "/targets/target-1/packages");
    expect(screen.getByText("inventory error")).toBeInTheDocument();
  });

  it("renders LXC inventory, node filtering and loading/error states", () => {
    mocks.containers.data = [container, { ...container, id: 102, node_id: "node-2", name: "db-lxc" }];
    renderPage(<ContainersPage />, "/containers?node=node-1");
    expect(screen.getByText("Gefiltert nach ausgewähltem Node")).toBeInTheDocument();
    expect(screen.getByText("web-lxc")).toBeInTheDocument();
    expect(screen.queryByText("db-lxc")).not.toBeInTheDocument();
    cleanup();
    mocks.containers.data = [];
    renderPage(<ContainersPage />);
    expect(screen.getByText(/Keine LXC-Container entdeckt/)).toBeInTheDocument();
    cleanup();
    mocks.containers.isLoading = true;
    renderPage(<ContainersPage />);
    expect(screen.getByText("Container werden geladen…")).toBeInTheDocument();
    cleanup();
    mocks.containers.isLoading = false;
    mocks.containers.error = new Error("container failure");
    renderPage(<ContainersPage />);
    expect(screen.getByText("container failure")).toBeInTheDocument();
  });

  it("renders schedule empty and populated states", () => {
    mocks.targets.data = [target];
    mocks.updatePolicies.data = [
      { id: "safe", enabled: true, maximum_risk: "low", allowed_packages: ["curl"] },
      { id: "paused", enabled: false, maximum_risk: "high", allowed_packages: [] },
    ];
    renderPage(<SchedulesPage />);
    expect(screen.getByRole("heading", { name: "Zeitpläne" })).toBeInTheDocument();
    expect(screen.getByText("Noch keine Zeitpläne")).toBeInTheDocument();
    cleanup();
    mocks.schedules.data = [
      { id: "nightly", operation: "collect_package_inventory", timezone: "Europe/Berlin", target_ids: [target.id], every_minutes: 60, enabled: true, last_run_at: null, next_run_at: "2026-01-01T01:00:00Z", last_error: null },
      { id: "weekly", operation: "health_check", timezone: "UTC", target_ids: [target.id], every_minutes: 120, enabled: false, last_run_at: "2026-01-01T00:00:00Z", next_run_at: "2026-01-01T02:00:00Z", last_error: "worker offline" },
    ];
    renderPage(<SchedulesPage />);
    expect(screen.getByText("nightly")).toBeInTheDocument();
    expect(screen.getAllByText("Aktiv").length).toBeGreaterThan(0);
    expect(screen.getByText("worker offline")).toBeInTheDocument();
    expect(screen.getAllByText("Pausiert").length).toBeGreaterThan(0);
  });

  it("validates, submits and reports schedule form states", async () => {
    mocks.targets.data = [target];
    renderPage(<SchedulesPage />);
    const submit = screen.getByRole("button", { name: /Zeitplan speichern/ });
    expect(submit).toBeDisabled();
    await userEvent.type(screen.getByPlaceholderText("nightly-inventory"), "nightly");
    await userEvent.selectOptions(screen.getAllByRole("combobox", { name: "Ziel" })[0], target.id);
    expect(submit).toBeEnabled();
    fireEvent.click(screen.getByLabelText("Schwellwert aktivieren"));
    await userEvent.click(submit);
    await waitFor(() => expect(mocks.createSchedule).toHaveBeenCalledWith(expect.objectContaining({ id: "nightly", target_ids: [target.id], every_minutes: 60, threshold: { metric: "cpu_basis_points", operator: "greater_than_or_equal", value: 8000 } })));
    cleanup();
    mocks.schedules.isLoading = true;
    renderPage(<SchedulesPage />);
    expect(screen.getByText("Zeitpläne werden geladen…")).toBeInTheDocument();
    cleanup();
    mocks.schedules.isLoading = false;
    mocks.schedules.error = new Error("schedule failure");
    renderPage(<SchedulesPage />);
    expect(screen.getByText("schedule failure")).toBeInTheDocument();
  });

  it("creates an update policy and converts its maintenance window to minutes", async () => {
    mocks.targets.data = [target];
    renderPage(<UpdatePoliciesPage />);
    await userEvent.type(screen.getByPlaceholderText("security-updates"), "security");
    await userEvent.selectOptions(screen.getAllByRole("combobox", { name: "Freigegebenes Ziel" })[0], target.id);
    await userEvent.type(screen.getByPlaceholderText("curl, openssl"), "curl, openssl");
    fireEvent.change(screen.getByDisplayValue("00:00"), { target: { value: "01:00" } });
    fireEvent.change(screen.getByDisplayValue("23:59"), { target: { value: "02:00" } });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Maximales Risiko" }), "medium");
    await userEvent.click(screen.getByRole("button", { name: "Policy speichern" }));
    await waitFor(() => expect(mocks.createUpdatePolicy).toHaveBeenCalledWith(expect.objectContaining({
      id: "security",
      allowed_targets: [target.id],
      allowed_packages: ["curl", "openssl"],
      maintenance_start_minute: 60,
      maintenance_end_minute: 120,
      maximum_risk: "medium",
    })));
  });

  it("lists policies with their target, packages, risk and UTC maintenance window", () => {
    mocks.targets.data = [target];
    mocks.updatePolicies.data = [{ id: "security", enabled: true, allowed_targets: [target.id], allowed_packages: ["curl", "openssl"], maintenance_start_minute: 60, maintenance_end_minute: 120, timezone: "UTC", maximum_risk: "medium" }];
    renderPage(<UpdatePoliciesPage />);
    expect(screen.getByRole("heading", { name: "Update-Policies" })).toBeInTheDocument();
    expect(screen.getAllByText("test-target").length).toBeGreaterThan(0);
    expect(screen.getByText("curl, openssl")).toBeInTheDocument();
    expect(screen.getByText((_text, element) => element?.tagName === "DD" && element.textContent === "01:00–02:00")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Zu den Zeitplänen/ })).toHaveAttribute("href", "/schedules");
  });

  it("deletes a policy only after confirmation and keeps the system policy protected", async () => {
    mocks.targets.data = [target];
    mocks.updatePolicies.data = [
      { id: "standard-all-packages-legacy", enabled: true, allowed_targets: [target.id], allowed_packages: [], maintenance_start_minute: 0, maintenance_end_minute: 1439, timezone: "UTC", maximum_risk: "high" },
      { id: "lxcup-standard-all-packages", enabled: true, allowed_targets: [target.id], allowed_packages: [], maintenance_start_minute: 0, maintenance_end_minute: 1439, timezone: "UTC", maximum_risk: "high" },
    ];
    renderPage(<UpdatePoliciesPage />);

    expect(screen.getAllByRole("button", { name: "Policy löschen" })).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Policy löschen" }));

    expect(window.confirm).toHaveBeenCalledWith(expect.stringContaining("standard-all-packages-legacy"));
    await waitFor(() => expect(mocks.deleteUpdatePolicy).toHaveBeenCalledWith("standard-all-packages-legacy", true));
  });

  it("links to policy management when creating a package-update schedule", async () => {
    mocks.targets.data = [target];
    renderPage(<SchedulesPage />);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Aufgabe" }), "update_packages");
    expect(screen.getByRole("link", { name: /Zuerst eine Policy erstellen/ })).toHaveAttribute("href", "/update-policies");
  });

  it("discovers and manages docker workloads", async () => {
    mocks.containers.data = [container];
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    await userEvent.selectOptions(screen.getByRole("combobox"), "101");
    expect(screen.getByText(/Noch keine Docker-Container/)).toBeInTheDocument();
    mocks.workloads.data = [{ host_container_id: 101, id: "docker-1", name: "web", image: "nginx", state: "running", status: "Up", ports: ["80/tcp"], started_at: null, labels: ["app=web"], presence: "present", change_state: "new", management_state: "discovered", discovered_at: "2026-01-01" }];
    cleanup();
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    await userEvent.selectOptions(screen.getByRole("combobox"), "101");
    expect(await screen.findByText("web")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Aufnehmen" }));
    await waitFor(() => expect(mocks.adoptDockerWorkload).toHaveBeenCalledWith(101, "docker-1"));
    await userEvent.click(screen.getByRole("button", { name: "Entfernen" }));
    await userEvent.click(screen.getByRole("button", { name: /Ja, Inventareintrag/ }));
    await waitFor(() => expect(mocks.removeDockerWorkload).toHaveBeenCalledWith(101, "docker-1"));
  });

  it("offers manually registered LXC targets with a connected agent for Docker discovery", async () => {
    mocks.targets.data = [{ ...target, state: "managed" }];
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    expect(screen.getByRole("option", { name: "test-target · 192.0.2.10" })).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "LXC mit Agent" }), "target:target-1");
    await userEvent.click(screen.getByRole("button", { name: "Docker-Container entdecken" }));
    await waitFor(() => expect(mocks.discoverTargetDocker).toHaveBeenCalledWith("target-1"));
    expect(await screen.findByText("web")).toBeInTheDocument();
  });

  it("covers loading, error and filtered empty inventory states", async () => {
    mocks.targets.isLoading = true;
    renderPage(<Dashboard />);
    expect(screen.getByText("Ressourcen werden geladen…")).toBeInTheDocument();
    cleanup();
    mocks.targets.isLoading = false;
    mocks.targets.error = new Error("target failure");
    renderPage(<Dashboard />);
    expect(screen.getByText("Ressourcen konnten nicht geladen werden: target failure")).toBeInTheDocument();
    cleanup();
    mocks.containers.data = [container];
    mocks.workloads.isLoading = true;
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "101" } });
    expect(screen.getByText("Docker-Inventar wird geladen…")).toBeInTheDocument();
    cleanup();
    mocks.workloads.isLoading = false;
    mocks.workloads.error = new Error("docker failure");
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "101" } });
    expect(screen.getByText("docker failure")).toBeInTheDocument();
  });

  it("shows Docker discovery audit status, host, and reconciliation changes", async () => {
    mocks.containers.data = [container];
    mocks.dockerDiscovery.data = { id: "run-1", host_container_id: 101, status: "failed", started_at: "2026-01-01T00:00:00Z", finished_at: "2026-01-01T00:00:01Z", container_count: 0, error_code: "docker_unavailable" };
    mocks.workloads.data = [{ host_container_id: 101, id: "docker-1", name: "web", image: "nginx:1", state: "exited", status: "Exited (0)", ports: [], started_at: null, labels: [], presence: "present", change_state: "changed", management_state: "discovered", discovered_at: "2026-01-01T00:00:00Z" }];
    renderPage(<DockerPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "101" } });

    expect(screen.getByText(/Host: web-lxc/)).toBeInTheDocument();
    expect(screen.getByText(/Fehler: docker_unavailable/)).toBeInTheDocument();
    expect(screen.getAllByText("web-lxc").length).toBeGreaterThan(0);
    expect(screen.getByText(/exited · Exited/)).toBeInTheDocument();
    expect(screen.getByText(/geändert · entdeckt/)).toBeInTheDocument();
  });

  it("renders a successful discovery and unchanged or missing managed workloads", () => {
    mocks.containers.data = [container];
    mocks.dockerDiscovery.data = { id: "run-ok", host_container_id: 101, status: "succeeded", started_at: "2026-01-01T00:00:00Z", finished_at: "2026-01-01T00:00:01Z", container_count: 2, error_code: null };
    mocks.workloads.data = [
      { host_container_id: 101, id: "docker-managed", name: "managed-web", image: "nginx:1", state: "running", status: "Up", ports: [], started_at: null, labels: [], presence: "present", change_state: "unchanged", management_state: "managed", discovered_at: "2026-01-01T00:00:00Z" },
      { host_container_id: 101, id: "docker-missing", name: "old-web", image: "nginx:1", state: "exited", status: "Exited", ports: [], started_at: null, labels: [], presence: "missing", change_state: "missing", management_state: "discovered", discovered_at: "2026-01-01T00:00:00Z" },
    ];
    renderPage(<DockerPage />, "/docker?host=101");

    expect(screen.getByText(/Status:/)).toHaveTextContent("succeeded");
    expect(screen.getByText("unverändert · aufgenommen")).toBeInTheDocument();
    expect(screen.getByText("verschwunden · nicht mehr vorhanden")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Aufnehmen" })).not.toBeInTheDocument();
  });

  it("opens Docker inventory with the selected host from a target deep link", async () => {
    mocks.containers.data = [container];
    renderPage(<DockerPage />, "/docker?host=101");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getByText("web-lxc")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    expect(screen.getByRole("combobox")).toHaveValue("101");
    expect(screen.getByText(/Für diesen Host gibt es noch keinen protokollierten Discovery-Lauf/)).toBeInTheDocument();
  });
});

describe("onboarding and secret pages", () => {
  it("keeps resource registration forms collapsed until the matching add button is clicked", async () => {
    const sections = [
      { area: "linux_server" as const, button: "Hinzufügen", heading: "Serverzugang konfigurieren" },
      { area: "lxc" as const, button: "Hinzufügen", heading: "LXC-Container hinzufügen" },
      { area: "windows_server" as const, button: "Hinzufügen", heading: "Windows-Zugang konfigurieren" },
    ];

    for (const section of sections) {
      renderPage(<TargetsPage area={section.area} />);
      expect(screen.queryByText("Verbindungsdaten")).not.toBeInTheDocument();
      const toggle = screen.getByRole("button", { name: section.button });
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      await userEvent.click(toggle);
      expect(screen.getByRole("heading", { name: section.heading })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Schließen" })).toHaveAttribute("aria-expanded", "true");
      await userEvent.click(screen.getByRole("button", { name: "Schließen" }));
      expect(screen.queryByText("Verbindungsdaten")).not.toBeInTheDocument();
      cleanup();
    }
  });

  it("keeps the server and LXC add-form expansion state independent", async () => {
    renderPage(<TargetAreaSwitcher />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    expect(screen.getByRole("button", { name: "Schließen" })).toHaveAttribute("aria-expanded", "true");

    await userEvent.click(screen.getByRole("button", { name: "LXC-Bereich" }));
    expect(screen.getByRole("button", { name: "Hinzufügen" })).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));

    await userEvent.click(screen.getByRole("button", { name: "Serverbereich" }));
    expect(screen.getByRole("button", { name: "Schließen" })).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(screen.getByRole("button", { name: "LXC-Bereich" }));
    expect(screen.getByRole("button", { name: "Schließen" })).toHaveAttribute("aria-expanded", "true");
  });

  it("creates and rotates a secret", async () => {
    renderPage(<SecretsPage />);
    expect(await screen.findByText("SSH-Passwort")).toBeInTheDocument();
    expect(screen.getAllByText("Aktiv").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Umfang: Global").length).toBeGreaterThan(0);
    const inputs = screen.getAllByRole("textbox");
    await userEvent.type(inputs[0], "new-secret");
    await userEvent.type(screen.getByLabelText("Secret-Wert"), "value");
    await userEvent.click(screen.getByRole("button", { name: "Secret speichern" }));
    await userEvent.click(screen.getAllByRole("button", { name: "Rotieren" })[0]);
    await userEvent.type(screen.getByPlaceholderText("Neuen Secret-Wert eingeben"), "rotated");
    await userEvent.click(screen.getByRole("button", { name: "Rotation bestätigen" }));
    expect(mocks.createSecret).toHaveBeenCalled();
  });

  it("links token reconfiguration audit events to their deployment job", async () => {
    mocks.listSecretAudit.mockResolvedValue([{ secret_id: "agent", action: "agent_reconfiguration_queued", role: "admin", occurred_at: "2026-01-01T00:00:00Z", related_job_id: "job-deploy" }] as any);
    renderPage(<SecretsPage />);
    expect(await screen.findByRole("link", { name: /Workflow öffnen/ })).toHaveAttribute("href", "/workflows/job-deploy");
  });

  it("registers a target using an inline generated secret", async () => {
    mocks.targets.data = [];
    renderPage(<TargetsPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    const fields = screen.getAllByRole("textbox");
    await userEvent.type(fields[0], "new-target");
    await userEvent.type(screen.getByPlaceholderText("IP oder DNS-Name …"), "192.0.2.20");
    const selects = screen.getAllByRole("combobox");
    await userEvent.selectOptions(selects[1], "cred");
    await userEvent.selectOptions(selects[2], "known");
    await userEvent.selectOptions(selects[3], "agent");
    await userEvent.click(screen.getAllByRole("button", { name: "＋ Neu" })[0]);
    await userEvent.type(screen.getByPlaceholderText("z. B. lxcup-test-ssh"), "inline");
    await userEvent.click(screen.getByRole("button", { name: "Wert erzeugen" }));
    await userEvent.click(screen.getByRole("button", { name: "Secret erstellen und auswählen" }));
    await waitFor(() => expect(mocks.createSecret).toHaveBeenCalled());
  });

  it("copies the complete target host preparation script including curl installation", async () => {
    const script = '#!/usr/bin/env bash\ninstall_package() { apt-get install --yes "$package"; }\nuseradd --create-home --shell /bin/bash "${username}"\n';
    const fetchScript = vi.fn(async () => ({ ok: true, text: async () => script }));
    vi.stubGlobal("fetch", fetchScript);
    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    renderPage(<TargetsPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    await userEvent.click(screen.getByRole("button", { name: "Vorbereitungsskript kopieren" }));
    expect(fetchScript).toHaveBeenCalledWith("/bootstrap-lxcup-user.sh");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(script));
    expect(screen.getByRole("button", { name: /Skript kopiert/ })).toBeInTheDocument();
  });

  it("renders pending targets and submits a target with onboarding disabled", async () => {
    mocks.targets.data = [{ ...target, state: "pending" }];
    renderPage(<TargetsPage />);
    await userEvent.click(screen.getByRole("button", { name: "Hinzufügen" }));
    const fields = screen.getAllByRole("textbox");
    await userEvent.type(fields[0], "target-2");
    await userEvent.type(screen.getByPlaceholderText("IP oder DNS-Name …"), "192.0.2.21");
    const selects = screen.getAllByRole("combobox");
    fireEvent.change(selects[1], { target: { value: "cred" } });
    fireEvent.change(selects[2], { target: { value: "known" } });
    fireEvent.change(selects[3], { target: { value: "agent" } });
    fireEvent.submit(screen.getByRole("button", { name: "Hinzufügen" }).closest("form")!);
    await waitFor(() => expect(mocks.createTarget).toHaveBeenCalled());
  });

  it("resumes visible onboarding progress and links each workflow after a page reload", () => {
    mocks.targets.data = [target];
    mocks.jobs.data = [
      { id: "job-inventory", operation: "collect_package_inventory", target: { target: target.id }, status: "queued", created_at: "2026-01-01T00:03:00Z" },
      { id: "job-health", operation: "health_check", target: { target: target.id }, status: "succeeded", created_at: "2026-01-01T00:02:00Z" },
      { id: "job-deploy", operation: "deploy_agent", target: { target: target.id }, status: "succeeded", created_at: "2026-01-01T00:01:00Z" },
    ];
    renderPage(<TargetsPage />);

    expect(screen.getByRole("link", { name: "Agent · Erfolgreich" })).toHaveAttribute("href", "/workflows/job-deploy");
    expect(screen.getByRole("link", { name: "Healthcheck · Erfolgreich" })).toHaveAttribute("href", "/workflows/job-health");
    expect(screen.getByRole("link", { name: "Paketinventar · Wartet" })).toHaveAttribute("href", "/workflows/job-inventory");
  });

  it("shows the version reported by a connected target agent", () => {
    mocks.targets.data = [{ ...target, agent_version: "0.3.1" }];
    renderPage(<TargetsPage />);
    expect(screen.getByText("v0.3.1")).toBeInTheDocument();
  });

  it("uses the same resource-card inventory design for LXC, Linux, and Windows", () => {
    const resources = [
      { area: "lxc" as const, kind: "lxc" as const, badge: "LXC" },
      { area: "linux_server" as const, kind: "linux_server" as const, badge: "Linux" },
      { area: "windows_server" as const, kind: "windows_server" as const, badge: "Win" },
    ];

    for (const resource of resources) {
      mocks.targets.data = [{ ...target, kind: resource.kind, agent_version: "0.3.1" }];
      renderPage(<TargetsPage area={resource.area} />);

      expect(screen.getByRole("list", { name: "Ressourcen" })).toBeInTheDocument();
      expect(screen.getByText(resource.badge)).toBeInTheDocument();
      expect(screen.getByText("v0.3.1")).toBeInTheDocument();
      expect(screen.getByRole("link", { name: "Paketinventar für test-target" })).toHaveAttribute("href", "/targets/target-1/packages");
      expect(screen.queryByRole("table")).not.toBeInTheDocument();
      cleanup();
    }
  });

  it("shows package inventory and telemetry health in each resource card", () => {
    mocks.targets.data = [target];
    mocks.packageInventory.data = { target_id: target.id, status: "complete", collected_at: "2099-01-01T00:00:00Z", packages: [
      { name: "curl", installed_version: "8.5.0", candidate_version: "8.5.0", architecture: "amd64", source: "apt" },
      { name: "openssl", installed_version: "3.0.1", candidate_version: "3.0.2", architecture: "amd64", source: "apt" },
    ] };
    mocks.telemetry.data = { target_id: target.id, collected_at: "2099-01-01T00:00:00Z", samples: [{ collected_at: "2099-01-01T00:00:00Z", cpu_basis_points: 2500, memory_basis_points: 4000, storage_basis_points: 5000, load_1_milli: 100, network_rx_bytes: 10, network_tx_bytes: 20, process_count: 5 }] };

    renderPage(<TargetsPage area="lxc" />);

    expect(screen.getByRole("link", { name: "Paketinventar für test-target" })).toHaveAttribute("href", "/targets/target-1/packages");
    expect(screen.getByText(/2 Pakete/)).toBeInTheDocument();
    expect(screen.getByText("1 Update verfügbar")).toBeInTheDocument();
    const telemetryLink = screen.getByRole("link", { name: "Systemauslastung für test-target" });
    expect(telemetryLink).toHaveAttribute("href", "/targets/target-1");
    expect(screen.getByText(/Aktuell/)).toBeInTheDocument();
    expect(within(telemetryLink).getByText(/CPU 25.0%/)).toBeInTheDocument();
    expect(within(telemetryLink).getByText(/RAM 40.0%/)).toBeInTheDocument();
    expect(within(telemetryLink).getByText(/Speicher 50.0%/)).toBeInTheDocument();
  });

  it("starts enrollment and displays a failed state", async () => {
    mocks.containers.data = [container];
    mocks.targets.data = [target];
    mocks.createEnrollment.mockImplementation(async () => {
      mocks.enrollment.data = { id: "enrollment-1", container_id: 101, state: "failed", failure_reason: "Agent nicht erreichbar", created_at: "2026-01-01", updated_at: "2026-01-01" };
      return { id: "enrollment-1" };
    });
    renderPage(<EnrollmentPage />);
    const enrollmentSelects = screen.getAllByRole("combobox");
    await userEvent.selectOptions(enrollmentSelects[0], "101");
    await userEvent.selectOptions(enrollmentSelects[1], "target-1");
    await userEvent.click(screen.getByRole("button", { name: "LXC aufnehmen" }));
    await waitFor(() => expect(mocks.createEnrollment).toHaveBeenCalledWith(101, "target-1", true));
    expect(await screen.findByText("Agent nicht erreichbar")).toBeInTheDocument();
  });

  it("renders a connected enrollment without automatic healthcheck", async () => {
    mocks.containers.data = [container];
    mocks.targets.data = [target];
    mocks.createEnrollment.mockImplementation(async () => {
      mocks.enrollment.data = { id: "enrollment-1", container_id: 101, state: "connected", failure_reason: null, created_at: "2026-01-01", updated_at: "2026-01-01" };
      return { id: "enrollment-1" };
    });
    renderPage(<EnrollmentPage />);
    const selects = screen.getAllByRole("combobox");
    fireEvent.change(selects[0], { target: { value: "101" } });
    fireEvent.change(selects[1], { target: { value: "target-1" } });
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "LXC aufnehmen" }));
    expect(await screen.findByText(/Agent verbunden/)).toBeInTheDocument();
  });

  it("chains health success into an idempotent package inventory workflow", async () => {
    mocks.containers.data = [container];
    mocks.targets.data = [target];
    mocks.job.data = { id: "job-health", operation: "health_check", status: "succeeded", target: { target: "target-1" } };
    mocks.createEnrollment.mockImplementation(async () => {
      mocks.enrollment.data = { id: "enrollment-1", container_id: 101, state: "connected", failure_reason: null, created_at: "2026-01-01", updated_at: "2026-01-01" };
      return { id: "enrollment-1" };
    });
    renderPage(<EnrollmentPage />);
    const selects = screen.getAllByRole("combobox");
    await userEvent.selectOptions(selects[0], "101");
    await userEvent.selectOptions(selects[1], "target-1");
    await userEvent.click(screen.getByRole("button", { name: "LXC aufnehmen" }));
    await waitFor(() => expect(mocks.createAnsibleJob).toHaveBeenCalledWith(expect.objectContaining({ operation: "health_check" })));
    await waitFor(() => expect(mocks.createAnsibleJob).toHaveBeenCalledWith(expect.objectContaining({ operation: "collect_package_inventory", idempotency_key: "enrollment-packages-enrollment-1" })));
    expect(await screen.findByText("Protokoll öffnen")).toBeInTheDocument();
  });

  it("shows enrollment empty and submission error paths", async () => {
    renderPage(<EnrollmentPage />);
    expect(screen.getByText("Keine laufenden, noch auswählbaren LXCs entdeckt.")).toBeInTheDocument();
    cleanup();
    mocks.containers.data = [container];
    mocks.createEnrollment.mockRejectedValue(new Error("enrollment failed"));
    renderPage(<EnrollmentPage />);
    fireEvent.change(screen.getAllByRole("combobox")[0], { target: { value: "101" } });
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "LXC aufnehmen" }));
    expect(await screen.findByText("enrollment failed")).toBeInTheDocument();
  });
});

describe("workflow pages", () => {
  it("starts a confirmed package workflow and lists failure codes", async () => {
    mocks.targets.data = [target];
    mocks.updatePolicies.data = [{ id: "safe-packages", allowed_targets: ["target-1"], allowed_packages: ["nginx", "curl"], maintenance_start_minute: 0, maintenance_end_minute: 1439, timezone: "UTC", maximum_risk: "high", enabled: true }] as any;
    mocks.jobs.data = [{ id: "job-1", operation: "update_packages", playbook: "packages/update.yml", playbook_version: "1", target: { target: "target-1" }, mode: "apply", status: "failed", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" }] as any;
    mocks.events.data = [{ sequence: 1, job_id: "job-1", event: { kind: "failed", code: "playbook_failed" }, created_at: "2026-01-01T00:00:00Z" }] as any;
    renderPage(<WorkflowsPage />);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Ziel" }), "target-1");
    await userEvent.selectOptions(screen.getAllByRole("combobox")[1], "update_packages");
    await userEvent.selectOptions(screen.getAllByRole("combobox")[2], "plan");
    await userEvent.type(screen.getByPlaceholderText("z. B. nginx,curl"), "nginx,curl");
    await userEvent.selectOptions(screen.getAllByRole("combobox")[3], "safe-packages");
    await userEvent.click(screen.getByRole("checkbox", { name: /Ich bestätige Ziel und den Umfang dieser Vorschau/ }));
    await userEvent.click(screen.getByRole("button", { name: "Workflow starten" }));
    await waitFor(() => expect(mocks.createAnsibleJob).toHaveBeenCalled());
    expect(screen.getByText("playbook_failed")).toBeInTheDocument();
    const protocolTable = screen.getByRole("table");
    expect(protocolTable).toHaveTextContent("test-target");
    expect(protocolTable).toHaveTextContent("Pakete aktualisieren");
    expect(screen.getByText("1 fehlgeschlagen")).toBeInTheDocument();
    expect(screen.queryByText(/In diesem Entwicklungs-Stack ist derzeit kein ausführender Ansible-Worker gestartet/)).not.toBeInTheDocument();
  });

  it("renders workflow logs and copies the technical log", async () => {
    mocks.targets.data = [target];
    mocks.job.data = { id: "job-1", operation: "deploy_agent", playbook: "agent/deploy.yml", playbook_version: "1", target: { target: "target-1" }, mode: "apply", status: "succeeded", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    mocks.events.data = [{ sequence: 1, job_id: "job-1", event: { kind: "worker_log", source: "stdout", message: "ok" }, created_at: "2026-01-01T00:00:00Z" }] as any;
    Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => undefined) } });
    renderPage(<WorkflowDetailPage />, "/workflows/job-1");
    expect(screen.getByRole("heading", { name: "Agent installieren" })).toBeInTheDocument();
    expect(screen.getAllByText("test-target").length).toBeGreaterThan(0);
    expect(screen.getByRole("listitem")).toHaveClass("grid");
    expect(await screen.findByText("Vollständiger technischer Log")).toBeInTheDocument();
    expect(screen.getByText("Ungekürzte Controller- und Workerausgabe zum Kopieren und Debuggen.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Log kopieren" }));
    expect(navigator.clipboard.writeText).toHaveBeenCalled();
  });

  it("retries a failed workflow by requeueing its existing job", async () => {
    mocks.job.data = { id: "job-failed", operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { target: "target-1" }, mode: "check", status: "failed", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    renderPage(<WorkflowDetailPage />, "/workflows/job-failed");
    await userEvent.click(screen.getByRole("button", { name: "Fehlgeschlagenen Job erneut versuchen" }));
    await waitFor(() => expect(mocks.retryAnsibleJob).toHaveBeenCalledWith("job-failed", false));
  });

  it("covers queued, failed and reconcile workflow guidance and event details", () => {
    mocks.job.data = { id: "job-q", operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { node: "node-1" }, mode: "check", status: "queued", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    renderPage(<WorkflowDetailPage />, "/workflows/job-q");
    expect(screen.getByText("Wartet auf den Ansible-Worker")).toBeInTheDocument();
    cleanup();
    mocks.job.data = { ...mocks.job.data, id: "job-f", status: "failed", target: { container: 101 } };
    mocks.events.data = [
      { sequence: 1, job_id: "job-f", event: { kind: "queued" }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 2, job_id: "job-f", event: { kind: "status_changed", status: "failed" }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 3, job_id: "job-f", event: { kind: "task_started", task: "Gather" }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 4, job_id: "job-f", event: { kind: "task_finished", task: "Gather", changed: false }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 5, job_id: "job-f", event: { kind: "failed" }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 6, job_id: "job-f", event: { kind: "reconcile_required" }, created_at: "2026-01-01T00:00:00Z" },
      { sequence: 7, job_id: "job-f", event: { kind: "worker_log", message: undefined }, created_at: "2026-01-01T00:00:00Z" },
    ] as any;
    renderPage(<WorkflowDetailPage />, "/workflows/job-f");
    expect(screen.getAllByText("Workflow fehlgeschlagen").length).toBeGreaterThan(0);
    cleanup();
    mocks.job.data = { ...mocks.job.data, id: "job-r", status: "reconcile_required", target: { target: "target-1" } };
    renderPage(<WorkflowDetailPage />, "/workflows/job-r");
    expect(screen.getAllByText("Abgleich erforderlich").length).toBeGreaterThan(0);
  });

  it("shows safe recovery guidance for a changed SSH host key", () => {
    mocks.job.data = { id: "job-host-key", operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { target: "target-1" }, mode: "check", status: "failed", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    mocks.events.data = [{ sequence: 1, job_id: "job-host-key", event: { kind: "failed", code: "host_key_changed" }, created_at: "2026-01-01T00:00:00Z" }] as any;
    renderPage(<WorkflowDetailPage />, "/workflows/job-host-key");
    expect(screen.getByText("SSH-Host-Key stimmt nicht überein")).toBeInTheDocument();
    expect(screen.getByText(/Deaktiviere die Host-Key-Prüfung nicht/)).toBeInTheDocument();
  });

  it("renders workflow loading and API error states", () => {
    mocks.jobs.isLoading = true;
    renderPage(<WorkflowsPage />);
    expect(screen.getByText("Lade Workflows…")).toBeInTheDocument();
    cleanup();
    mocks.jobs.isLoading = false;
    mocks.jobs.error = new Error("workflow failure");
    renderPage(<WorkflowsPage />);
    expect(screen.getByText("workflow failure")).toBeInTheDocument();
    cleanup();
    mocks.job.isLoading = true;
    renderPage(<WorkflowDetailPage />, "/workflows/job-1");
    expect(screen.getByText("Lade Workflow…")).toBeInTheDocument();
    cleanup();
    mocks.job.isLoading = false;
    mocks.job.error = new Error("job missing");
    renderPage(<WorkflowDetailPage />, "/workflows/job-1");
    expect(screen.getByText("job missing")).toBeInTheDocument();
    cleanup();
    mocks.job.error = null;
    mocks.job.data = { id: "job-e", operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { target: "target-1" }, mode: "check", status: "succeeded", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    mocks.events.error = new Error("event stream failed");
    renderPage(<WorkflowDetailPage />, "/workflows/job-e");
    expect(screen.getByText("event stream failed")).toBeInTheDocument();
  });
});

describe("application shell", () => {
  it("keeps the activity sidebar preference after reloading the app", async () => {
    window.localStorage.removeItem("lxcup-activity-sidebar-open");
    renderPage(<App />);
    await screen.findByRole("link", { name: "lxcup Übersicht" });
    const sidebar = document.querySelector('aside[aria-label="Aktivitäten"]');
    expect(sidebar).not.toHaveClass("translate-x-full");
    await userEvent.click(screen.getByRole("button", { name: "Aktivitäten ausblenden" }));
    expect(window.localStorage.getItem("lxcup-activity-sidebar-open")).toBe("false");

    cleanup();
    renderPage(<App />);
    await screen.findByRole("link", { name: "lxcup Übersicht" });
    expect(screen.getByRole("button", { name: "Aktivitäten einblenden" })).toBeInTheDocument();
    expect(document.querySelector('aside[aria-label="Aktivitäten"]')).toHaveClass("translate-x-full");
  });

  it("asks for a bearer token and exposes the verified read-only role", async () => {
    mocks.getSession.mockRejectedValueOnce(new ApiError("authentication required", 401, "unauthorized"));
    mocks.getSession.mockResolvedValueOnce({ role: "viewer", expires_in_seconds: 3600 } as any);
    renderPage(<App />);
    expect(await screen.findByRole("heading", { name: "Bei lxcup anmelden" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("API-Token"), { target: { value: "viewer-token" } });
    await userEvent.click(screen.getByRole("button", { name: "Anmelden" }));
    expect(await screen.findByText("Viewer-Zugriff: Änderungen und Workflow-Ausführungen sind deaktiviert.")).toBeInTheDocument();
    expect(mocks.setCredentials).toHaveBeenLastCalledWith("viewer-token", "viewer");
  });

  it("renders navigation and an SSE error notification", async () => {
    mocks.subscribe.mockImplementation(((onEvent: any, onError: any) => {
      onEvent({ type: "Status", payload: { resource: "node", resource_id: "node-1", state: "online" } });
      onEvent({ type: "Status", payload: { resource: "ansible_job", resource_id: "job-1", state: "failed" } });
      onEvent({ type: "Error", payload: { code: "worker_error", message: "Worker offline" } });
      onError();
      return () => undefined;
    }) as any);
    mocks.targets.data = [target];
    mocks.workerAvailability.data = { available: false, last_seen_at: null };
    mocks.jobs.data = [{ id: "job-alert", operation: "deploy_agent", playbook: "agent/deploy.yml", playbook_version: "1", target: { target: "target-1" }, mode: "apply", status: "failed", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" }, { id: "job-reconcile", operation: "health_check", playbook: "health.yml", playbook_version: "1", target: { target: "target-1" }, mode: "check", status: "reconcile_required", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" }] as any;
    renderPage(<App />);
    await screen.findByRole("link", { name: "lxcup Übersicht" });
    expect(screen.getByRole("link", { name: "lxcup Übersicht" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Server" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "LXC-Container" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Docker-Container" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Windows" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Tasks & Workflows" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Secrets" })).toBeInTheDocument();
    expect(screen.getByText("Ansible-Worker nicht verfügbar")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Zugänge & Agenten" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /LXC einbinden/ })).not.toBeInTheDocument();
    expect(mocks.subscribe).toHaveBeenCalled();
    expect(await screen.findByText(/Echtzeitverbindung unterbrochen/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Benachrichtigungen" }));
    expect(document.querySelector("header")).toHaveClass("z-[60]");
    expect(document.querySelector('aside[aria-label="Aktivitäten"]')).toHaveClass("z-40");
    expect(screen.getAllByText("Fehlgeschlagen").length).toBeGreaterThan(0);
    const notificationPanel = screen.getByText("Benachrichtigungen").closest("div.absolute") as HTMLElement;
    const agentNotification = within(notificationPanel).getByRole("link", { name: /Agent installieren/ });
    expect(agentNotification).toHaveTextContent("test-target");
    expect(agentNotification).toHaveTextContent("lxc · 192.0.2.10");
    expect(agentNotification).toHaveTextContent("Apply");
    expect(agentNotification).toHaveTextContent("Job · job-ale");
    await userEvent.click(screen.getByRole("button", { name: "Alle als gelesen markieren" }));
    expect(screen.queryByText("2")).not.toBeInTheDocument();
    await userEvent.click(agentNotification);
    await userEvent.click(screen.getByRole("button", { name: "Theme wechseln" }));
    cleanup();
    renderPage(<App />, "/unknown");
    const breadcrumbs = await screen.findByRole("navigation", { name: "Brotkrumennavigation" });
    expect(within(breadcrumbs).getByText("Seite nicht gefunden")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Seite nicht gefunden" })).toBeInTheDocument();
  });

  it("shows telemetry alerts with resource context and marks recovered alerts resolved", async () => {
    window.localStorage.removeItem("lxcup-read-notifications");
    window.localStorage.removeItem("lxcup-telemetry-alert-history");
    mocks.targets.data = [target];
    mocks.telemetryAlerts.data = [{ id: "target-1:cpu", target_id: "target-1", target_name: "test-target", target_kind: "lxc", target_address: "192.0.2.10", metric: "CPU", severity: "warning", value_basis_points: 9_200, threshold_basis_points: 9_000, age_seconds: null, triggered_at: "2026-09-26T14:00:00Z", observed_at: "2026-09-26T14:05:00Z" }];
    renderPage(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "Benachrichtigungen" }));
    const alert = await screen.findByRole("link", { name: /Hohe CPU-Auslastung/ });
    expect(alert).toHaveAttribute("href", "/targets/target-1");
    expect(alert).toHaveTextContent("test-target");
    expect(alert).toHaveTextContent("92.0% · Grenzwert 90.0%");

    mocks.telemetryAlerts.data = [];
    await userEvent.click(screen.getByRole("button", { name: "Alle als gelesen markieren" }));
    expect(await screen.findByText("Behoben")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Hohe CPU-Auslastung/ })).toHaveTextContent("Auslastung wieder im Normalbereich.");
  });
});
