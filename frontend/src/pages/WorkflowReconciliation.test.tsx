import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  job: undefined as Record<string, unknown> | undefined,
  reconcile: vi.fn(async () => ({ id: "reconcile-job", reconciles_job_id: "source-job" })),
}));

vi.mock("../api", async () => {
  const actual = await vi.importActual<typeof import("../api")>("../api");
  return { ...actual, reconcileAnsibleJob: mocks.reconcile };
});

vi.mock("../queries", () => ({
  queryKeys: {
    ansibleJob: (id: string) => ["ansible-job", id],
    ansibleJobEvents: (id: string) => ["ansible-job-events", id],
    ansibleJobs: ["ansible-jobs"],
  },
  useAnsibleJob: () => ({ data: mocks.job, isLoading: false, error: null }),
  useAnsibleJobEvents: () => ({ data: [], isLoading: false, error: null }),
  useContainers: () => ({ data: [], isLoading: false, error: null }),
  useTargets: () => ({ data: [], isLoading: false, error: null }),
}));

import { WorkflowDetailPage } from "./WorkflowDetailPage";
import { ApiError } from "../api";

function renderWorkflow() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/workflows/source-job"]}>
        <Routes><Route path="/workflows/:jobId" element={<WorkflowDetailPage />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Workflow reconciliation actions", () => {
  beforeEach(() => {
    mocks.reconcile.mockClear();
    mocks.job = {
      id: "source-job",
      operation: "repair_agent",
      playbook: "agent/repair.yml",
      playbook_version: "1",
      target: { target: "target-1" },
      mode: "apply",
      status: "reconcile_required",
      parameter_hash: "hash",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    };
  });

  it("starts only the controlled reconciliation for an unresolved Apply and links the child job", async () => {
    renderWorkflow();
    fireEvent.click(screen.getByRole("button", { name: "Ist-Zustand abgleichen" }));
    await waitFor(() => expect(mocks.reconcile).toHaveBeenCalledWith("source-job"));
    expect(await screen.findByRole("link", { name: "Abgleich öffnen" })).toHaveAttribute("href", "/workflows/reconcile-job");
  });

  it("links a reconciliation job back to its source and does not offer another reconcile action", () => {
    mocks.job = { ...mocks.job, id: "reconcile-job", mode: "reconcile", status: "succeeded", reconciles_job_id: "source-job" };
    renderWorkflow();
    expect(screen.getByRole("link", { name: "source-job" })).toHaveAttribute("href", "/workflows/source-job");
    expect(screen.queryByRole("button", { name: "Ist-Zustand abgleichen" })).not.toBeInTheDocument();
  });

  it("links the blocking active job and its status when reconciliation is rejected", async () => {
    mocks.reconcile.mockRejectedValueOnce(new ApiError(
      "another job is executing for this target",
      409,
      "ansible_target_busy",
      undefined,
      false,
      { id: "blocking-job", status: "applying" },
    ));
    renderWorkflow();
    fireEvent.click(screen.getByRole("button", { name: "Ist-Zustand abgleichen" }));
    const blocker = await screen.findByRole("link", { name: "blocking-job" });
    expect(blocker).toHaveAttribute("href", "/workflows/blocking-job");
    expect(screen.getByText(/Blockierender Workflow \(Läuft\)/)).toBeInTheDocument();
  });
});
