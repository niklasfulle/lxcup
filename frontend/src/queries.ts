import { useQuery } from "@tanstack/react-query";
import { apiClient, getAnsibleJob, getAnsibleJobEvents, getDockerDiscovery, getEnrollment, getPackageInventory, getTargetTelemetry, getWorkerAvailability, listAnsibleJobs, listDockerWorkloads, listSchedules, listTargets, listUpdatePolicies, type AnsibleJobDto, type AnsibleJobEvent, type ContainerDto, type DockerDiscoveryRunDto, type DockerWorkloadDto, type EnrollmentDto, type ExecutionSafetyDto, type PackageInventoryDto, type ScheduleDto, type TargetDto, type TelemetryDto, type UpdatePolicyDto, type WorkerAvailabilityDto } from "./api";

export const queryKeys = {
  targets: ["targets"] as const,
  containers: ["containers"] as const,
  executionSafety: (executionId: string) => ["executions", executionId, "safety"] as const,
  ansibleJobs: ["ansible-jobs"] as const,
  ansibleJob: (jobId: string) => ["ansible-jobs", jobId] as const,
  ansibleJobEvents: (jobId: string) => ["ansible-jobs", jobId, "events"] as const,
  workerAvailability: ["ansible-worker-availability"] as const,
  dockerWorkloads: (containerId: number) => ["containers", containerId, "docker-workloads"] as const,
  dockerDiscovery: (containerId: number) => ["containers", containerId, "docker-discovery"] as const,
  packageInventory: (targetId: string) => ["targets", targetId, "package-inventory"] as const,
  telemetry: (targetId: string) => ["targets", targetId, "telemetry"] as const,
  schedules: ["schedules"] as const,
  updatePolicies: ["update-policies"] as const,
};

export function useTargets() {
  return useQuery<TargetDto[]>({
    queryKey: queryKeys.targets,
    queryFn: ({ signal }) => listTargets(signal),
    staleTime: 15_000,
    refetchInterval: (query) => {
      const targets = query.state.data;
      if (!targets?.length) return false;
      return targets.some((target) => target.state === "pending") ? 3_000 : 30_000;
    },
  });
}

export function useExecutionSafety(executionId: string | undefined) {
  return useQuery<ExecutionSafetyDto>({
    queryKey: executionId ? queryKeys.executionSafety(executionId) : ["executions", "none", "safety"],
    queryFn: ({ signal }) => apiClient.get<ExecutionSafetyDto>(`/api/v1/executions/${executionId}/safety`, signal),
    enabled: Boolean(executionId),
    staleTime: 5_000,
  });
}

export function useContainers() {
  return useQuery<ContainerDto[]>({
    queryKey: queryKeys.containers,
    queryFn: ({ signal }) => apiClient.get<ContainerDto[]>("/api/v1/containers", signal),
    staleTime: 15_000,
  });
}

export function useEnrollment(enrollmentId: string | undefined) {
  return useQuery<EnrollmentDto>({
    queryKey: enrollmentId ? ["enrollments", enrollmentId] : ["enrollments", "none"],
    queryFn: ({ signal }) => getEnrollment(enrollmentId!, signal),
    enabled: Boolean(enrollmentId),
    refetchInterval: (query) => query.state.data?.state === "connected" || query.state.data?.state === "failed" ? false : 1_500,
  });
}

function isActiveJob(status: string | undefined) {
  return status === "queued" || status === "checking" || status === "planned" || status === "applying" || status === "reconcile_required";
}

export function useAnsibleJobs() {
  return useQuery<AnsibleJobDto[]>({
    queryKey: queryKeys.ansibleJobs,
    queryFn: ({ signal }) => listAnsibleJobs(signal),
    refetchInterval: (query) => query.state.data?.some((job) => isActiveJob(job.status)) ? 2_000 : false,
  });
}

export function useWorkerAvailability() {
  return useQuery<WorkerAvailabilityDto>({
    queryKey: queryKeys.workerAvailability,
    queryFn: ({ signal }) => getWorkerAvailability(signal),
    staleTime: 2_000,
    refetchInterval: 5_000,
  });
}

export function useAnsibleJob(jobId: string | undefined) {
  return useQuery<AnsibleJobDto>({
    queryKey: jobId ? queryKeys.ansibleJob(jobId) : ["ansible-jobs", "none"],
    queryFn: ({ signal }) => getAnsibleJob(jobId!, signal),
    enabled: Boolean(jobId),
    refetchInterval: (query) => isActiveJob(query.state.data?.status) ? 2_000 : false,
  });
}

export function useAnsibleJobEvents(jobId: string | undefined, active: boolean) {
  return useQuery<AnsibleJobEvent[]>({
    queryKey: jobId ? queryKeys.ansibleJobEvents(jobId) : ["ansible-jobs", "none", "events"],
    queryFn: ({ signal }) => getAnsibleJobEvents(jobId!, signal),
    enabled: Boolean(jobId),
    refetchInterval: active ? 2_000 : false,
  });
}

export function useDockerWorkloads(containerId: number | undefined) {
  return useQuery<DockerWorkloadDto[]>({
    queryKey: containerId ? queryKeys.dockerWorkloads(containerId) : ["containers", "none", "docker-workloads"],
    queryFn: ({ signal }) => listDockerWorkloads(containerId!, signal),
    enabled: Boolean(containerId),
    staleTime: 10_000,
  });
}

export function useDockerDiscovery(containerId: number | undefined) {
  return useQuery<DockerDiscoveryRunDto | null>({
    queryKey: containerId ? queryKeys.dockerDiscovery(containerId) : ["containers", "none", "docker-discovery"],
    queryFn: ({ signal }) => getDockerDiscovery(containerId!, signal),
    enabled: Boolean(containerId),
    staleTime: 5_000,
  });
}

export function usePackageInventory(targetId: string | undefined) {
  return useQuery<PackageInventoryDto>({
    queryKey: targetId ? queryKeys.packageInventory(targetId) : ["targets", "none", "package-inventory"],
    queryFn: ({ signal }) => getPackageInventory(targetId!, signal),
    enabled: Boolean(targetId),
    staleTime: 10_000,
  });
}

export function useTargetTelemetry(targetId: string | undefined) {
  return useQuery<TelemetryDto>({ queryKey: targetId ? queryKeys.telemetry(targetId) : ["targets", "none", "telemetry"], queryFn: ({ signal }) => getTargetTelemetry(targetId!, signal), enabled: Boolean(targetId), staleTime: 2_000, refetchInterval: 5_000 });
}

export function useSchedules() {
  return useQuery<ScheduleDto[]>({ queryKey: queryKeys.schedules, queryFn: ({ signal }) => listSchedules(signal), staleTime: 10_000 });
}

export function useUpdatePolicies() {
  return useQuery<UpdatePolicyDto[]>({ queryKey: queryKeys.updatePolicies, queryFn: ({ signal }) => listUpdatePolicies(signal), staleTime: 10_000 });
}
