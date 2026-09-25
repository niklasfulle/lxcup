export type ApiEnvelope<T> = { data: T; request_id: string };

export type ContainerDto = {
  id: number;
  target_id?: string | null;
  node_id: string;
  name: string;
  operating_system: string;
  status: string;
  management_state: string;
  discovered_at: string;
};
export type TargetKind = "lxc" | "linux_server" | "windows_server";
export type TargetTransport = "ssh" | "winrm";
export type TargetState = "pending" | "managed" | "disabled";
export type TargetDto = { id: string; name: string; kind: TargetKind; address: string; transport: TargetTransport; ssh_user?: string | null; credential_secret_ref: string; ssh_known_hosts_secret_ref?: string | null; agent_secret_ref: string; state: TargetState; agent_version?: string | null; created_at: string; updated_at: string };
export type CreateTargetRequest = { name: string; kind: TargetKind; address: string; transport: TargetTransport; ssh_user?: string | null; credential_secret_ref: string; ssh_known_hosts_secret_ref?: string | null; agent_secret_ref: string };
export type EnrollmentState = "requested" | "discovering" | "installing_agent" | "registering_agent" | "connected" | "failed" | "disabled";
export type EnrollmentDto = { id: string; container_id: number; target_id: string | null; state: EnrollmentState; failure_reason: string | null; created_at: string; updated_at: string };

export type ContainerAction = "start" | "stop" | "shutdown" | "reboot" | "refresh" | "clone" | "backup" | "restore" | "delete";
export type ContainerActionTaskDto = {
  id: string;
  container_id: number;
  action: ContainerAction;
  status: "queued" | "running" | "succeeded" | "failed" | "blocked";
  idempotency_key: string;
  error: string | null;
  created_at: string;
  updated_at: string;
};
export type AgentHealthDto = { healthy: boolean; info: { agent_id: string; platform: string; hostname: string; version: string; protocol_version: string }; metrics: { collected_at: string; commands_total: number; commands_failed: number; last_command_at: string | null } };
export type WorkerAvailabilityDto = { available: boolean; last_seen_at: string | null };

export type ScanDto = {
  id: string;
  container_id: number;
  status: string;
  update_count: number;
  created_at: string;
  completed_at: string | null;
};

export type PlanDto = {
  id: string;
  container_id: number;
  status: string;
  requested_packages: string[];
  change_count: number;
  plan_hash: string | null;
  created_at: string;
};

export type ExecutionDto = {
  id: string;
  plan_id: string;
  status: string;
  started_at: string | null;
  finished_at: string | null;
};

export type ExecutionSafetyDto = {
  snapshot: "NotRequested" | { Succeeded: { task_id: string } } | { Failed: { reason: string } };
  healthchecks: Array<{ check: { kind: string; [key: string]: unknown }; healthy: boolean; detail: string; checked_at: string }>;
  reboot: "Required" | "NotRequired" | "Unknown";
  automatic_rollback: boolean;
};

export type AnsibleOperation = "deploy_agent" | "update_agent" | "repair_agent" | "update_packages" | "configure_target" | "health_check" | "collect_package_inventory";
export type AnsibleExecutionMode = "check" | "plan" | "apply" | "reconcile";
export type AnsibleJobDto = {
  id: string;
  operation: AnsibleOperation;
  playbook: string;
  playbook_version: string;
  target: { container: number } | { node: string } | { target: string };
  mode: AnsibleExecutionMode;
  status: string;
  parameter_hash: string;
  package_names?: string[];
  update_policy_id?: string;
  approved_plan_job_id?: string;
  reconciles_job_id?: string;
  created_at: string;
  updated_at: string;
};

export type AnsibleJobEvent = {
  sequence: number;
  job_id: string;
  event: {
    kind: "queued" | "status_changed" | "task_started" | "task_finished" | "failed" | "reconcile_required" | "worker_log";
    status?: string;
    task?: string;
    changed?: boolean;
    code?: string;
    source?: string;
    message?: string;
  };
  created_at: string;
};

export type DockerWorkloadDto = {
  host_container_id: number;
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  ports: string[];
  started_at: string | null;
  labels: string[];
  presence: "present" | "missing";
  change_state: "new" | "changed" | "unchanged" | "missing";
  management_state: "discovered" | "managed";
  discovered_at: string;
};

export type DockerDiscoveryRunDto = {
  id: string;
  host_container_id: number;
  status: "running" | "succeeded" | "failed";
  started_at: string;
  finished_at: string | null;
  container_count: number;
  error_code: string | null;
};

export type DockerDiscoveryResultDto = { run: DockerDiscoveryRunDto; workloads: DockerWorkloadDto[] };
export type PackageInventoryDto = {
  target_id: string;
  status: "complete" | "not_collected";
  collected_at: string | null;
  packages: Array<{ name: string; installed_version: string; architecture: string | null; source: string | null }>;
};
export type TelemetryDto = { target_id: string; collected_at: string | null; samples: Array<{ collected_at: string; cpu_basis_points: number | null; memory_basis_points: number | null; storage_basis_points: number | null; load_1_milli: number | null; network_rx_bytes: number | null; network_tx_bytes: number | null; process_count: number | null }> };
export type ThresholdRuleDto = { metric: "cpu_basis_points" | "memory_basis_points" | "storage_basis_points" | "heartbeat_age_seconds"; operator: "greater_than_or_equal" | "less_than_or_equal"; value: number };
export type ScheduleDto = { id: string; operation: string; timezone: string; target_ids: string[]; every_minutes: number; enabled: boolean; threshold: ThresholdRuleDto | null; policy_id: string | null; last_run_at: string | null; next_run_at: string; last_error: string | null };
export type UpdatePolicyDto = { id: string; allowed_targets: string[]; allowed_packages: string[]; maintenance_start_minute: number; maintenance_end_minute: number; timezone: string; maximum_risk: "low" | "medium" | "high"; enabled: boolean };

export type CreateAnsibleJobRequest = {
  operation: AnsibleOperation;
  target_id?: string;
  /** Legacy field for pre-target APIs; new callers use target_id. */
  container_id?: number;
  mode: AnsibleExecutionMode;
  parameters: Record<string, unknown>;
  idempotency_key: string;
  confirmed: boolean;
  policy_id?: string;
  approved_plan_job_id?: string;
};

export type SecretKind = "ssh_private_key" | "ssh_password" | "ssh_known_hosts" | "agent_token" | "generic";
export type SecretScope = { type: "global" } | { type: "node"; id: string } | { type: "container"; id: number };
export type SecretMetadata = {
  metadata: {
    metadata: {
      id: string;
      name: string;
      kind: SecretKind;
      scope: SecretScope;
      created_at: string;
      updated_at: string;
    };
    status: "active" | "revoked";
  };
};
export type SecretAuditEvent = { secret_id: string; action: string; role: string; occurred_at: string; related_job_id?: string | null };

export type ApiErrorBody = {
  error?: { code: string; message: string };
  request_id?: string;
};
export type AuthRole = "viewer" | "operator" | "admin";
export type AuthSession = { role: AuthRole; expires_in_seconds: number | null };

export type ApiEvent =
  | { type: "Task"; payload: { task_id: string; state: string } }
  | { type: "Log"; payload: { source: string; message: string } }
  | { type: "Status"; payload: { resource: string; resource_id: string; state: string } }
  | { type: "Error"; payload: { code: string; message: string; request_id?: string } };

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly code: string,
    public readonly requestId?: string,
    public readonly retryable = false,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export class ApiClient {
  private token: string | null = null;
  private role: AuthRole | null = null;
  private unauthorizedHandler: (() => void) | null = null;

  constructor(private readonly baseUrl = import.meta.env.VITE_API_BASE_URL ?? "") {}

  setCredentials(token: string | null, role: AuthRole | null = null) {
    this.token = token;
    this.role = token === null ? null : role;
  }

  setUnauthorizedHandler(handler: (() => void) | null) {
    this.unauthorizedHandler = handler;
  }

  async getSession(signal?: AbortSignal) {
    return this.get<AuthSession>("/api/v1/auth/session", signal);
  }

  async logout(signal?: AbortSignal) {
    return this.post<void>("/api/v1/auth/logout", undefined, signal);
  }

  async get<T>(path: string, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, { signal });
  }

  async post<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, {
      method: "POST",
      signal,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  }

  async delete<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, {
      method: "DELETE",
      signal,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  }

  subscribe(onEvent: (event: ApiEvent) => void, onError?: () => void): () => void {
    let stopped = false;
    let retryTimer: ReturnType<typeof globalThis.setTimeout> | undefined;
    let controller: AbortController | undefined;
    const connect = async () => {
      controller = new AbortController();
      try {
        const headers = new Headers({ accept: "text/event-stream" });
        if (this.token) headers.set("authorization", `Bearer ${this.token}`);
        const response = await fetch(`${this.baseUrl}/api/v1/events`, { headers, cache: "no-store", signal: controller.signal });
        if (response.status === 401) {
          this.unauthorizedHandler?.();
          stopped = true;
          return;
        }
        if (!response.ok || !response.body) throw new ApiError("Die Echtzeitverbindung wurde abgelehnt.", response.status, "event_stream_error");
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (!stopped) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true }).replaceAll("\r\n", "\n");
          buffer = consumeEventFrames(buffer, onEvent, onError);
        }
      } catch (error) {
        if (!stopped && !(error instanceof DOMException && error.name === "AbortError")) onError?.();
      }
      if (!stopped) retryTimer = globalThis.setTimeout(() => void connect(), 1000);
    };
    void connect();
    return () => {
      stopped = true;
      if (retryTimer !== undefined) globalThis.clearTimeout(retryTimer);
      controller?.abort();
    };
  }

  async patch<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, {
      method: "PATCH",
      signal,
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const maxAttempts = 3;
    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      try {
        return await this.requestAttempt<T>(path, init, attempt, maxAttempts);
      } catch (error) {
        if (error instanceof ApiError || attempt + 1 >= maxAttempts) throw error;
        await delay(attempt);
      }
    }
    throw new ApiError("Die API-Anfrage ist fehlgeschlagen.", 0, "api_error", undefined, true);
  }

  private async requestAttempt<T>(path: string, init: RequestInit | undefined, attempt: number, maxAttempts: number): Promise<T> {
    const method = (init?.method ?? "GET").toUpperCase();
    this.assertRequestPermission(path, method, init);
    const headers = this.requestHeaders(init);
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      cache: "no-store",
      headers,
    });
    const text = await response.text();
    if (response.status === 204) return undefined as T;
    const payload = parsePayload<T>(text);
    if (!response.ok) {
      const error = payload as ApiErrorBody | undefined;
      const retryable = response.status >= 500;
      if (response.status === 401) this.unauthorizedHandler?.();
      if (retryable && attempt + 1 < maxAttempts) {
        await delay(attempt);
        return this.requestAttempt<T>(path, init, attempt + 1, maxAttempts);
      }
      throw new ApiError(error?.error?.message ?? "Die API-Anfrage ist fehlgeschlagen.", response.status, error?.error?.code ?? "api_error", error?.request_id, retryable);
    }
    if (payload === undefined || !("data" in payload)) {
      throw new ApiError("Die API hat eine ungültige Antwort geliefert.", response.status, "invalid_response");
    }
    return payload.data;
  }

  private assertRequestPermission(path: string, method: string, init: RequestInit | undefined) {
    const workflowOperation = readWorkflowOperation(path, method, init?.body);
    const readOnlyPost = isReadOnlyPost(path, workflowOperation);
    if (this.role === "viewer" && method !== "GET" && method !== "HEAD" && !readOnlyPost) {
      throw new ApiError("Deine Rolle darf keine Änderungen ausführen.", 403, "permission_denied");
    }
    if (this.role === "operator" && isAdminOnlyRequest(path, method)) {
      throw new ApiError("Für diese Aktion ist die Admin-Rolle erforderlich.", 403, "permission_denied");
    }
  }

  private requestHeaders(init: RequestInit | undefined) {
    const headers = new Headers(init?.headers);
    if (this.token) headers.set("authorization", `Bearer ${this.token}`);
    headers.set("accept", "application/json");
    return headers;
  }
}

function readWorkflowOperation(path: string, method: string, body: BodyInit | null | undefined) {
  if (path !== "/api/v1/ansible/jobs" || method !== "POST") return undefined;
  try { return parseWorkflowOperation(body); } catch { return undefined; }
}

function isReadOnlyPost(path: string, operation: string | undefined) {
  return path === "/api/v1/auth/logout"
    || (path === "/api/v1/ansible/jobs" && ["health_check", "collect_package_inventory"].includes(operation ?? ""))
    || path.endsWith("/scans")
    || /^\/api\/v1\/scans\/[^/]+\/run$/.test(path);
}

function isAdminOnlyRequest(path: string, method: string) {
  const destructive = method === "DELETE" || /\/(revoke|disable|abort)$/.test(path);
  return destructive || (path.startsWith("/api/v1/secrets") && method !== "GET");
}

function consumeEventFrames(
  buffer: string,
  onEvent: (event: ApiEvent) => void,
  onError?: () => void,
) {
  let boundary = buffer.indexOf("\n\n");
  while (boundary >= 0) {
    const frame = buffer.slice(0, boundary);
    buffer = buffer.slice(boundary + 2);
    const type = frame.split("\n").find((line) => line.startsWith("event:"))?.slice(6).trim();
    const data = frame.split("\n").filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trimStart()).join("\n");
    if (type && data) dispatchEventFrame(data, onEvent, onError);
    boundary = buffer.indexOf("\n\n");
  }
  return buffer;
}

function dispatchEventFrame(data: string, onEvent: (event: ApiEvent) => void, onError?: () => void) {
  try { onEvent(JSON.parse(data) as ApiEvent); } catch { onError?.(); }
}

function parseWorkflowOperation(body: BodyInit | null | undefined): string | undefined {
  if (typeof body !== "string") return undefined;
  return (JSON.parse(body) as { operation?: string }).operation;
}

function parsePayload<T>(text: string): ApiEnvelope<T> | ApiErrorBody | undefined {
  if (!text) return undefined;
  try {
    return JSON.parse(text) as ApiEnvelope<T> | ApiErrorBody;
  } catch {
    return undefined;
  }
}

function delay(attempt: number): Promise<void> {
  return new Promise((resolve) => globalThis.setTimeout(resolve, 150 * 2 ** attempt));
}

export const apiClient = new ApiClient();

export function listTargets(signal?: AbortSignal) { return apiClient.get<TargetDto[]>("/api/v1/targets", signal); }
export function createTarget(request: CreateTargetRequest, signal?: AbortSignal) { return apiClient.post<TargetDto>("/api/v1/targets", request, signal); }
export function getPackageInventory(targetId: string, signal?: AbortSignal) { return apiClient.get<PackageInventoryDto>(`/api/v1/targets/${targetId}/package-inventory`, signal); }
export function getTargetTelemetry(targetId: string, signal?: AbortSignal) { return apiClient.get<TelemetryDto>(`/api/v1/targets/${targetId}/telemetry`, signal); }
export function listSchedules(signal?: AbortSignal) { return apiClient.get<ScheduleDto[]>("/api/v1/schedules", signal); }
export function createSchedule(request: Omit<ScheduleDto, "last_run_at" | "next_run_at" | "last_error">, signal?: AbortSignal) { return apiClient.post<ScheduleDto>("/api/v1/schedules", request, signal); }
export function setScheduleEnabled(id: string, enabled: boolean, signal?: AbortSignal) { return apiClient.patch<ScheduleDto>(`/api/v1/schedules/${encodeURIComponent(id)}`, { enabled }, signal); }
export function listUpdatePolicies(signal?: AbortSignal) { return apiClient.get<UpdatePolicyDto[]>("/api/v1/update-policies", signal); }
export function createUpdatePolicy(request: Omit<UpdatePolicyDto, "enabled"> & { enabled?: boolean }, signal?: AbortSignal) { return apiClient.post<UpdatePolicyDto>("/api/v1/update-policies", request, signal); }

export function createAnsibleJob(request: CreateAnsibleJobRequest, signal?: AbortSignal) {
  return apiClient.post<AnsibleJobDto>("/api/v1/ansible/jobs", request, signal);
}

export function listAnsibleJobs(signal?: AbortSignal) { return apiClient.get<AnsibleJobDto[]>("/api/v1/ansible/jobs", signal); }
export function getWorkerAvailability(signal?: AbortSignal) { return apiClient.get<WorkerAvailabilityDto>("/api/v1/ansible/worker-availability", signal); }
export function getAnsibleJob(id: string, signal?: AbortSignal) { return apiClient.get<AnsibleJobDto>(`/api/v1/ansible/jobs/${id}`, signal); }
export function getAnsibleJobEvents(id: string, signal?: AbortSignal) { return apiClient.get<AnsibleJobEvent[]>(`/api/v1/ansible/jobs/${id}/events`, signal); }
export function retryAnsibleJob(id: string, confirmed: boolean, signal?: AbortSignal) { return apiClient.post<AnsibleJobDto>(`/api/v1/ansible/jobs/${id}/retry`, { confirmed }, signal); }
export function reconcileAnsibleJob(id: string, signal?: AbortSignal) { return apiClient.post<AnsibleJobDto>(`/api/v1/ansible/jobs/${id}/reconcile`, {}, signal); }

export function createEnrollment(containerId: number, targetId: string | undefined, startOnboarding = true, idempotencyKey = crypto.randomUUID(), signal?: AbortSignal) {
  return apiClient.post<EnrollmentDto>("/api/v1/enrollments", { container_id: containerId, target_id: targetId, idempotency_key: idempotencyKey, start_onboarding: startOnboarding }, signal);
}

export function getEnrollment(id: string, signal?: AbortSignal) {
  return apiClient.get<EnrollmentDto>(`/api/v1/enrollments/${id}`, signal);
}

export function listSecrets(signal?: AbortSignal) {
  return apiClient.get<SecretMetadata[]>("/api/v1/secrets", signal);
}

export function listSecretAudit(signal?: AbortSignal) {
  return apiClient.get<SecretAuditEvent[]>("/api/v1/secrets/audit", signal);
}

export function createSecret(request: { name: string; kind: SecretKind; scope: SecretScope; value: string }, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>("/api/v1/secrets", request, signal);
}

export function rotateSecret(id: string, value: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>(`/api/v1/secrets/${id}/rotate`, { value, confirmed }, signal);
}

export function revokeSecret(id: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>(`/api/v1/secrets/${id}/revoke`, { confirmed }, signal);
}

export function deleteSecret(id: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.delete<void>(`/api/v1/secrets/${id}`, { confirmed }, signal);
}

export function createContainerAction(containerId: number, action: ContainerAction, confirmed = false, signal?: AbortSignal) {
  return apiClient.post<ContainerActionTaskDto>(`/api/v1/containers/${containerId}/actions`, { action, confirmed, idempotency_key: crypto.randomUUID() }, signal);
}

export function getAgentHealth(containerId: number, signal?: AbortSignal) {
  return apiClient.get<AgentHealthDto>(`/api/v1/containers/${containerId}/agent/health`, signal);
}

export function getAgentMetrics(containerId: number, signal?: AbortSignal) {
  return apiClient.get<AgentHealthDto["metrics"]>(`/api/v1/containers/${containerId}/agent/metrics`, signal);
}

export function listDockerWorkloads(containerId: number, signal?: AbortSignal) {
  return apiClient.get<DockerWorkloadDto[]>(`/api/v1/containers/${containerId}/docker/containers`, signal);
}

export function discoverDockerWorkloads(containerId: number, signal?: AbortSignal) {
  return apiClient.post<DockerDiscoveryResultDto>(`/api/v1/containers/${containerId}/docker/discover`, undefined, signal);
}

export function getDockerDiscovery(containerId: number, signal?: AbortSignal) {
  return apiClient.get<DockerDiscoveryRunDto | null>(`/api/v1/containers/${containerId}/docker/discovery`, signal);
}

export function adoptDockerWorkload(containerId: number, dockerId: string, signal?: AbortSignal) {
  return apiClient.post<DockerWorkloadDto>(`/api/v1/containers/${containerId}/docker/containers/${encodeURIComponent(dockerId)}/adopt`, undefined, signal);
}

export function removeDockerWorkload(containerId: number, dockerId: string, signal?: AbortSignal) {
  return apiClient.delete<void>(`/api/v1/containers/${containerId}/docker/containers/${encodeURIComponent(dockerId)}`, { confirmed: true }, signal);
}
