import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContainerDto, DockerWorkloadDto, SecretMetadata, TargetDto } from "./api";

const mocks = vi.hoisted(() => ({
  targets: { data: [] as TargetDto[], isLoading: false, error: null as Error | null },
  containers: { data: [] as ContainerDto[], isLoading: false, error: null as Error | null },
  jobs: { data: [], isLoading: false, error: null as Error | null },
  job: { data: undefined as any, isLoading: false, error: null as Error | null },
  events: { data: [], isLoading: false, error: null as Error | null },
  enrollment: { data: undefined as any, isLoading: false, error: null as Error | null },
  workloads: { data: [] as DockerWorkloadDto[], isLoading: false, error: null as Error | null },
  workerAvailability: { data: { available: true, last_seen_at: "2026-01-01T00:00:00Z" }, isLoading: false, error: null as Error | null },
  listSecrets: vi.fn(async () => [] as SecretMetadata[]),
  listSecretAudit: vi.fn(async () => []),
  createSecret: vi.fn(async (request: any) => ({ metadata: { metadata: { id: "secret-new", name: request.name, kind: request.kind, scope: request.scope, created_at: "2026-01-01", updated_at: "2026-01-01" }, status: "active" } })),
  createTarget: vi.fn(async () => ({ ...target, state: "pending" as const })),
  createAnsibleJob: vi.fn(async (request: any) => ({ id: request.operation === "health_check" ? "job-health" : "job-deploy", operation: request.operation, playbook: "agent/deploy.yml", playbook_version: "v1", target: { target: "target-1" }, mode: request.mode, status: "queued", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" })),
  createEnrollment: vi.fn(async () => ({ id: "enrollment-1" })),
  createContainerAction: vi.fn(async () => ({ id: "task-1", status: "queued" })),
  getAgentHealth: vi.fn(async () => ({ healthy: true, info: { agent_id: "agent-1", platform: "linux", hostname: "host", version: "0.1.0", protocol_version: "1" }, metrics: { collected_at: "2026-01-01", commands_total: 2, commands_failed: 0, last_command_at: null } })),
  getAgentMetrics: vi.fn(async () => ({ commands_total: 2, commands_failed: 0, last_command_at: null })),
  discoverDockerWorkloads: vi.fn(async () => undefined),
  adoptDockerWorkload: vi.fn(async () => undefined),
  removeDockerWorkload: vi.fn(async () => undefined),
  subscribe: vi.fn(() => () => undefined),
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
  queryKeys: { targets: ["targets"], nodes: ["nodes"], ansibleJobs: ["ansible-jobs"], containers: ["containers"], dockerWorkloads: (id: number) => ["docker", id] },
  useTargets: () => mocks.targets,
  useContainers: () => mocks.containers,
  useAnsibleJobs: () => mocks.jobs,
  useAnsibleJob: () => mocks.job,
  useAnsibleJobEvents: () => mocks.events,
  useEnrollment: () => mocks.enrollment,
  useDockerWorkloads: () => mocks.workloads,
  useWorkerAvailability: () => mocks.workerAvailability,
}));

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return { ...actual, listSecrets: mocks.listSecrets, listSecretAudit: mocks.listSecretAudit, createSecret: mocks.createSecret, createTarget: mocks.createTarget, createAnsibleJob: mocks.createAnsibleJob, createEnrollment: mocks.createEnrollment, createContainerAction: mocks.createContainerAction, getAgentHealth: mocks.getAgentHealth, getAgentMetrics: mocks.getAgentMetrics, discoverDockerWorkloads: mocks.discoverDockerWorkloads, adoptDockerWorkload: mocks.adoptDockerWorkload, removeDockerWorkload: mocks.removeDockerWorkload, apiClient: { subscribe: mocks.subscribe } };
});

import { Dashboard } from "./pages/Dashboard";
import { ContainerDetailPage } from "./pages/ContainerDetailPage";
import { DockerPage } from "./pages/DockerPage";
import { SecretsPage } from "./pages/SecretsPage";
import { TargetsPage, buildBootstrapCommand } from "./pages/TargetsPage";
import { EnrollmentPage } from "./pages/EnrollmentPage";
import { WorkflowsPage } from "./pages/WorkflowsPage";
import { WorkflowDetailPage } from "./pages/WorkflowDetailPage";
import App from "./App";
import { ResourceTree } from "./components/ResourceTree";

function renderPage(element: React.ReactElement, route = "/") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}><Routes><Route path="/containers/:containerId/*" element={element} /><Route path="/workflows/:jobId/*" element={element} /><Route path="*" element={element} /></Routes></MemoryRouter></QueryClientProvider>);
}

afterEach(() => cleanup());

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
  mocks.workerAvailability.data = { available: true, last_seen_at: "2026-01-01T00:00:00Z" };
  mocks.workloads.isLoading = false;
  mocks.workloads.error = null;
  mocks.listSecrets.mockResolvedValue([secret("cred", "ssh-password"), secret("known", "known-hosts", "ssh_known_hosts"), secret("agent", "agent-token", "agent_token")] as any);
  mocks.listSecretAudit.mockResolvedValue([{ secret_id: "cred", action: "created", role: "admin", occurred_at: "2026-01-01T00:00:00Z" }] as any);
  vi.clearAllMocks();
  Object.defineProperty(window, "confirm", { configurable: true, value: vi.fn(() => true) });
});

describe("inventory pages", () => {
  it("renders resource tree entries and empty state", () => {
    mocks.targets.data = [target];
    renderPage(<ResourceTree />);
    expect(screen.getByText("test-target")).toBeInTheDocument();
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

  it("renders dashboard data and empty state", () => {
    mocks.targets.data = [target];
    renderPage(<Dashboard />);
    expect(screen.getByRole("heading", { name: "Übersicht" })).toBeInTheDocument();
    expect(screen.getByText(/test-target \(managed\)/)).toBeInTheDocument();
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

  it("discovers and manages docker workloads", async () => {
    mocks.containers.data = [container];
    renderPage(<DockerPage />);
    await userEvent.selectOptions(screen.getByRole("combobox"), "101");
    expect(screen.getByText(/Noch keine Docker-Container/)).toBeInTheDocument();
    mocks.workloads.data = [{ host_container_id: 101, id: "docker-1", name: "web", image: "nginx", state: "running", status: "Up", management_state: "discovered", discovered_at: "2026-01-01" }];
    cleanup();
    renderPage(<DockerPage />);
    await userEvent.selectOptions(screen.getByRole("combobox"), "101");
    expect(await screen.findByText("web")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Aufnehmen" }));
    await waitFor(() => expect(mocks.adoptDockerWorkload).toHaveBeenCalledWith(101, "docker-1"));
    await userEvent.click(screen.getByRole("button", { name: "Entfernen" }));
    await userEvent.click(screen.getByRole("button", { name: /Ja, Inventareintrag/ }));
    await waitFor(() => expect(mocks.removeDockerWorkload).toHaveBeenCalledWith(101, "docker-1"));
  });

  it("covers loading, error and filtered empty inventory states", () => {
    mocks.targets.isLoading = true;
    renderPage(<Dashboard />);
    expect(screen.getAllByText("Daten werden geladen…").length).toBeGreaterThan(0);
    cleanup();
    mocks.targets.isLoading = false;
    mocks.targets.error = new Error("target failure");
    renderPage(<Dashboard />);
    expect(screen.getAllByText("target failure").length).toBeGreaterThan(0);
    cleanup();
    mocks.containers.data = [container];
    mocks.workloads.isLoading = true;
    renderPage(<DockerPage />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "101" } });
    expect(screen.getByText("Docker-Inventar wird geladen…")).toBeInTheDocument();
    cleanup();
    mocks.workloads.isLoading = false;
    mocks.workloads.error = new Error("docker failure");
    renderPage(<DockerPage />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "101" } });
    expect(screen.getByText("docker failure")).toBeInTheDocument();
  });
});

describe("onboarding and secret pages", () => {
  it("creates and rotates a secret", async () => {
    renderPage(<SecretsPage />);
    expect(await screen.findByText("ssh-password")).toBeInTheDocument();
    const inputs = screen.getAllByRole("textbox");
    await userEvent.type(inputs[0], "new-secret");
    await userEvent.type(screen.getByLabelText("Wert"), "value");
    await userEvent.click(screen.getByRole("button", { name: "Secret speichern" }));
    await userEvent.click(screen.getAllByRole("button", { name: "Rotieren" })[0]);
    await userEvent.type(screen.getByPlaceholderText("Neuer Wert"), "rotated");
    await userEvent.click(screen.getByRole("button", { name: "Bestätigen" }));
    expect(mocks.createSecret).toHaveBeenCalled();
  });

  it("registers a target using an inline generated secret", async () => {
    mocks.targets.data = [];
    renderPage(<TargetsPage />);
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

  it("copies the target host bootstrap command and installs curl when needed", async () => {
    expect(buildBootstrapCommand("http://192.168.1.20:5173")).toContain("bootstrap-lxcup-user.sh");
    expect(buildBootstrapCommand("http://192.168.1.20:5173")).toContain("apt-get install --yes curl");
    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    renderPage(<TargetsPage />);
    await userEvent.click(screen.getByRole("button", { name: /Installationsbefehl kopieren/ }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(expect.stringContaining("bootstrap-lxcup-user.sh")));
    expect(screen.getByRole("button", { name: /Befehl kopiert/ })).toBeInTheDocument();
  });

  it("renders pending targets and submits a target with onboarding disabled", async () => {
    mocks.targets.data = [{ ...target, state: "pending" }];
    renderPage(<TargetsPage />);
    const fields = screen.getAllByRole("textbox");
    await userEvent.type(fields[0], "target-2");
    await userEvent.type(screen.getByPlaceholderText("IP oder DNS-Name …"), "192.0.2.21");
    const selects = screen.getAllByRole("combobox");
    fireEvent.change(selects[1], { target: { value: "cred" } });
    fireEvent.change(selects[2], { target: { value: "known" } });
    fireEvent.change(selects[3], { target: { value: "agent" } });
    fireEvent.submit(screen.getByRole("button", { name: "Ziel registrieren" }).closest("form")!);
    await waitFor(() => expect(mocks.createTarget).toHaveBeenCalled());
  });

  it("shows the version reported by a connected target agent", () => {
    mocks.targets.data = [{ ...target, agent_version: "0.1.0" }];
    renderPage(<TargetsPage />);
    expect(screen.getByText("v0.1.0")).toBeInTheDocument();
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
    mocks.jobs.data = [{ id: "job-1", operation: "update_packages", playbook: "packages/update.yml", playbook_version: "1", target: { target: "target-1" }, mode: "apply", status: "failed", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" }] as any;
    mocks.events.data = [{ sequence: 1, job_id: "job-1", event: { kind: "failed", code: "playbook_failed" }, created_at: "2026-01-01T00:00:00Z" }] as any;
    renderPage(<WorkflowsPage />);
    await userEvent.selectOptions(screen.getAllByRole("combobox")[0], "target-1");
    await userEvent.selectOptions(screen.getAllByRole("combobox")[1], "update_packages");
    await userEvent.type(screen.getByPlaceholderText("z. B. nginx,curl"), "nginx,curl");
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: "Workflow starten" }));
    await waitFor(() => expect(mocks.createAnsibleJob).toHaveBeenCalled());
    expect(screen.getByText("playbook_failed")).toBeInTheDocument();
  });

  it("renders workflow logs and copies the technical log", async () => {
    mocks.job.data = { id: "job-1", operation: "deploy_agent", playbook: "agent/deploy.yml", playbook_version: "1", target: { target: "target-1" }, mode: "apply", status: "succeeded", parameter_hash: "hash", created_at: "2026-01-01", updated_at: "2026-01-01" };
    mocks.events.data = [{ sequence: 1, job_id: "job-1", event: { kind: "worker_log", source: "stdout", message: "ok" }, created_at: "2026-01-01T00:00:00Z" }] as any;
    Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => undefined) } });
    renderPage(<WorkflowDetailPage />, "/workflows/job-1");
    expect(await screen.findByText("Vollständiger technischer Log")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Log kopieren" }));
    expect(navigator.clipboard.writeText).toHaveBeenCalled();
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
    expect(screen.getByRole("link", { name: "lxcup Übersicht" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Server" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "LXC-Container" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Docker-Container" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Windows" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Tasks & Workflows" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Secrets" })).toBeInTheDocument();
    expect(screen.getByText("Ansible-Worker nicht verfügbar")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Zugänge & Agenten" })).not.toBeInTheDocument();
    expect(screen.getAllByText("＋ LXC einbinden").length).toBeGreaterThan(0);
    expect(mocks.subscribe).toHaveBeenCalled();
    expect(await screen.findByText(/Echtzeitverbindung unterbrochen/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Benachrichtigungen" }));
    expect(screen.getByText("Fehlgeschlagen")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Alle als gelesen markieren" }));
    expect(screen.queryByText("2")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: /deploy agent/ }));
    await userEvent.click(screen.getByRole("button", { name: "Theme wechseln" }));
    cleanup();
    renderPage(<App />, "/unknown");
    expect(screen.getByText("Seite nicht gefunden")).toBeInTheDocument();
  });
});
