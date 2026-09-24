import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import {
  useAnsibleJob,
  useAnsibleJobEvents,
  useAnsibleJobs,
  useDockerWorkloads,
  useEnrollment,
  useExecutionSafety,
  useTargets,
  useSchedules,
  useUpdatePolicies,
} from "./queries";
import * as api from "./api";

function Probe() {
  const targets = useTargets();
  const safety = useExecutionSafety("execution-1");
  const jobs = useAnsibleJobs();
  const job = useAnsibleJob("job-1");
  const events = useAnsibleJobEvents("job-1", false);
  const enrollment = useEnrollment("enrollment-1");
  const workloads = useDockerWorkloads(101);
  const schedules = useSchedules();
  const policies = useUpdatePolicies();
  return <output>{[targets, safety, jobs, job, events, enrollment, workloads, schedules, policies].filter((query) => query.isSuccess).length}</output>;
}

function DisabledProbe() {
  const safety = useExecutionSafety(undefined);
  const enrollment = useEnrollment(undefined);
  const job = useAnsibleJob(undefined);
  const events = useAnsibleJobEvents(undefined, true);
  const workloads = useDockerWorkloads(undefined);
  return <output>{[safety, enrollment, job, events, workloads].filter((query) => query.fetchStatus === "idle").length}</output>;
}

describe("query hooks", () => {
  it("load each resource through its API query function", async () => {
    vi.spyOn(api, "listTargets").mockResolvedValue([]);
    vi.spyOn(api.apiClient, "get").mockResolvedValue([] as never);
    vi.spyOn(api, "getAnsibleJob").mockResolvedValue({ id: "job-1" } as never);
    vi.spyOn(api, "getAnsibleJobEvents").mockResolvedValue([]);
    vi.spyOn(api, "getEnrollment").mockResolvedValue({ id: "enrollment-1", state: "connected" } as never);
    vi.spyOn(api, "listDockerWorkloads").mockResolvedValue([]);
    vi.spyOn(api, "listSchedules").mockResolvedValue([]);
    vi.spyOn(api, "listUpdatePolicies").mockResolvedValue([]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><Probe /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByText("9")).toBeInTheDocument());
  });

  it("keeps disabled queries idle and polls active target/job states", async () => {
    vi.spyOn(api, "listTargets").mockResolvedValue([{ id: "t", state: "pending" } as never]);
    vi.spyOn(api, "listAnsibleJobs").mockResolvedValue([{ id: "j", status: "applying" } as never]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><><DisabledProbe /><Probe /></></QueryClientProvider>);
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
  });
});
