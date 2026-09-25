import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { WorkflowsPage } from "./WorkflowsPage";

vi.mock("../api", () => ({ createAnsibleJob: vi.fn() }));
vi.mock("../queries", () => ({
  queryKeys: { ansibleJobs: ["ansible-jobs"] },
  useAnsibleJobs: () => ({ data: [], isLoading: false, error: null }),
  useAnsibleJobEvents: () => ({ data: [], isLoading: false, error: null }),
  useTargets: () => ({ data: [{ id: "target-1", name: "lxc-test", kind: "lxc", state: "managed" }] }),
  useUpdatePolicies: () => ({ data: [] }),
}));

describe("workflow mode contract", () => {
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
  });
});
