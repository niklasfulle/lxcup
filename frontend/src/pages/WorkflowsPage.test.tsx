import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { createAnsibleJob } from "../api";
import { buildWorkflowRequest, WorkflowsPage } from "./WorkflowsPage";
import workflowModeMatrix from "../../workflow-modes.json";

const mockTargets = vi.hoisted(() => ({ items: [
  { id: "target-1", name: "lxc-test", kind: "lxc", address: "192.0.2.1", state: "managed" },
  { id: "target-2", name: "linux-test", kind: "linux_server", address: "192.0.2.2", state: "managed" },
] }));

vi.mock("../api", () => ({ createAnsibleJob: vi.fn(async (request: any) => ({ id: `job-${request.target_id}`, operation: request.operation, playbook: "test.yml", playbook_version: "1", target: { target: request.target_id }, mode: request.mode, status: "queued", parameter_hash: "hash", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" })), getPackageInventory: vi.fn(async (target_id: string) => ({ target_id, status: "complete", collected_at: "2026-01-01T00:00:00Z", packages: [{ name: "curl", installed_version: "8.5.0", candidate_version: "8.6.0", architecture: "amd64", source: "stable" }] })) }));
vi.mock("../queries", () => ({
  queryKeys: { ansibleJobs: ["ansible-jobs"] },
  useAnsibleJobs: () => ({ data: [], isLoading: false, error: null }),
  useAnsibleJobEvents: () => ({ data: [], isLoading: false, error: null }),
  useTargets: () => ({ data: mockTargets.items }),
  useUpdatePolicies: () => ({ data: [] }),
}));

describe("workflow mode contract", () => {
  it("treats an empty package field as all available updates", () => {
    const request = buildWorkflowRequest("target-1", "update_packages", "plan", [], true, "standard");

    expect(request.parameters).toEqual({ operation: "update_packages", packages: ["*"] });
    expect(request.policy_id).toBe("standard");
  });

  it("passes only explicitly selected packages to a targeted plan", () => {
    const request = buildWorkflowRequest("target-1", "update_packages", "plan", ["curl", "nginx"], true, "standard");

    expect(request.parameters).toEqual({ operation: "update_packages", packages: ["curl", "nginx"] });
  });

  it("shows available updates in the picker and allows selecting them", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><WorkflowsPage /></MemoryRouter></QueryClientProvider>);
    fireEvent.change(screen.getByRole("combobox", { name: "Ziel" }), { target: { value: "target-1" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Operation" }), { target: { value: "update_packages" } });
    fireEvent.click(screen.getByRole("button", { name: "Updates auswählen" }));

    const dialog = await screen.findByRole("dialog", { name: "Verfügbare Updates" });
    expect(within(dialog).getByText("curl")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /curl/ }));
    expect(within(dialog).getByText("1 Paket für dieses Ziel ausgewählt")).toBeInTheDocument();
  });

  it("offers only modes allowed by the shared operation matrix", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><WorkflowsPage /></MemoryRouter></QueryClientProvider>);
    const operation = screen.getByRole("combobox", { name: "Operation" });
    const mode = screen.getByRole("combobox", { name: "Modus" });

    expect(within(mode).getAllByRole("option").map((option) => option.textContent)).toEqual(["Check"]);
    fireEvent.change(operation, { target: { value: "deploy_agent" } });
    expect(within(mode).getAllByRole("option").map((option) => option.textContent)).toEqual(["Check", "Plan / Dry-Run", "Apply"]);
    expect(within(mode).queryByRole("option", { name: "Reconcile" })).not.toBeInTheDocument();
    fireEvent.change(operation, { target: { value: "update_packages" } });
    expect(within(mode).getAllByRole("option").map((option) => option.textContent)).toEqual(["Plan / Dry-Run", "Apply"]);
    fireEvent.change(operation, { target: { value: "collect_package_inventory" } });
    expect(screen.getByRole("option", { name: "Paketinventar erfassen" })).toBeInTheDocument();
    expect(within(mode).getAllByRole("option").map((option) => option.textContent)).toEqual(["Check"]);
    expect(workflowModeMatrix.configure_target).toEqual(["check", "apply"]);
  });

  it("keeps Apply disabled until the user explicitly confirms the target and scope", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><WorkflowsPage /></MemoryRouter></QueryClientProvider>);
    fireEvent.change(screen.getByRole("combobox", { name: "Ziel" }), { target: { value: "target-1" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Operation" }), { target: { value: "deploy_agent" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Modus" }), { target: { value: "apply" } });

    const start = screen.getByRole("button", { name: /Workflow starten/ });
    expect(start).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /Ich bestätige Ziel, Umfang und Risiko dieser Änderung/ }));
    expect(start).toBeEnabled();
  });

  it("enqueues one workflow job per selected target and shows each result", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><WorkflowsPage /></MemoryRouter></QueryClientProvider>);
    fireEvent.click(screen.getByRole("tab", { name: "Bulk-Workflows" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Ziel auswählen: lxc-test" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Ziel auswählen: linux-test" }));
    fireEvent.click(screen.getByRole("button", { name: "2 Workflows starten" }));

    await waitFor(() => expect(createAnsibleJob).toHaveBeenCalledTimes(2));
    expect(vi.mocked(createAnsibleJob).mock.calls.map(([request]) => request.target_id).sort()).toEqual(["target-1", "target-2"]);
    expect(await screen.findByText("2 von 2 Workflows eingereiht")).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /Protokoll öffnen/ }).map((link) => link.getAttribute("href"))).toEqual([
      "/workflows/job-target-1",
      "/workflows/job-target-2",
    ]);
  });
});
