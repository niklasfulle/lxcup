import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AnsibleJobDto, AnsibleJobEvent } from "../api";
import { WorkflowDetailPage } from "./WorkflowDetailPage";

const preview = "TASK [Install service] changed: [target]\n--- before\n+++ after\n[REDACTED]";
const events = [
  {
    sequence: 1,
    created_at: "2026-09-25T12:00:00Z",
    event: { kind: "worker_log", source: "stdout", message: preview },
  },
  {
    sequence: 2,
    created_at: "2026-09-25T12:00:01Z",
    event: { kind: "worker_log", source: "plan", message: "Dry-Run erfolgreich: Ansible erwartet 1 Änderung. Es wurde nichts angewendet." },
  },
] as unknown as AnsibleJobEvent[];

vi.mock("../api", () => ({ retryAnsibleJob: vi.fn() }));
vi.mock("../queries", () => ({
  queryKeys: { ansibleJob: (id: string) => ["job", id], ansibleJobEvents: (id: string) => ["events", id], ansibleJobs: ["jobs"] },
  useAnsibleJob: () => ({
    data: {
      id: "job-1",
      operation: "deploy_agent",
      status: "succeeded",
      mode: "plan",
      target: { target: "target-1" },
      playbook: "agent/deploy.yml",
      playbook_version: "1",
      created_at: "2026-09-25T12:00:00Z",
      updated_at: "2026-09-25T12:00:01Z",
    } as unknown as AnsibleJobDto,
    isLoading: false,
    error: null,
  }),
  useAnsibleJobEvents: () => ({ data: events, isLoading: false, error: null }),
  useContainers: () => ({ data: [] }),
  useTargets: () => ({ data: [{ id: "target-1", name: "test-server", kind: "linux_server", address: "192.0.2.5" }] }),
}));

describe("workflow plan preview", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it("shows the saved dry-run output on reopening and copies only the redacted preview", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/workflows/job-1"]}><Routes><Route path="/workflows/:jobId" element={<WorkflowDetailPage />} /></Routes></MemoryRouter></QueryClientProvider>);

    expect(screen.getByRole("region", { name: "Plan-Vorschau" })).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Plan-Vorschau" })).getByText(/Ansible erwartet 1 Änderung/)).toBeInTheDocument();
    const previewElement = within(screen.getByRole("region", { name: "Plan-Vorschau" })).getByText((_, element) => element?.tagName === "PRE");
    expect(previewElement.textContent).toBe(preview);
    fireEvent.click(screen.getByRole("button", { name: "Vorschau kopieren" }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(preview);
    expect(screen.queryByText("the-secret-token")).not.toBeInTheDocument();
  });
});
