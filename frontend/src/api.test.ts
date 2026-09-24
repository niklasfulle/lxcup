import { describe, expect, it, vi } from "vitest";
import {
  ApiClient,
  ApiError,
  adoptDockerWorkload,
  createAnsibleJob,
  createContainerAction,
  createEnrollment,
  createSecret,
  createTarget,
  deleteSecret,
  discoverDockerWorkloads,
  getAgentHealth,
  getAgentMetrics,
  getAnsibleJob,
  getAnsibleJobEvents,
  getEnrollment,
  listAnsibleJobs,
  listDockerWorkloads,
  listSecretAudit,
  listSecrets,
  listTargets,
  removeDockerWorkload,
  revokeSecret,
  rotateSecret,
  apiClient,
} from "./api";
import { buildWorkflowRequest } from "./pages/WorkflowsPage";

describe("ApiClient", () => {
  it("unwraps a versioned API envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: [{ name: "pve01" }], request_id: "req-1" }), { status: 200 })));
    await expect(new ApiClient().get<{ name: string }[]>("/api/v1/targets")).resolves.toEqual([{ name: "pve01" }]);
  });

  it("exposes structured API errors without losing the request id", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: { code: "not_found", message: "container not found" }, request_id: "req-2" }), { status: 404 })));
    await expect(new ApiClient().get("/api/v1/containers")).rejects.toEqual(expect.objectContaining({ status: 404, code: "not_found", requestId: "req-2" }));
  });

  it("retries transient server failures before returning the envelope", async () => {
    vi.stubGlobal("fetch", vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: { code: "internal_error", message: "temporary" }, request_id: "req-3" }), { status: 503 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { ok: true }, request_id: "req-4" }), { status: 200 })));

    await expect(new ApiClient().get<{ ok: boolean }>("/api/v1/health")).resolves.toEqual({ ok: true });
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("rejects successful responses without the versioned data envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("not-json", { status: 200 })));

    await expect(new ApiClient().get("/api/v1/targets")).rejects.toEqual(expect.objectContaining({ code: "invalid_response", status: 200 }));
  });

  it("builds only allowlisted workflow fields without playbook or shell input", () => {
    const request = buildWorkflowRequest("target-101", "update_packages", "plan", "nginx, curl", false);

    expect(request).toMatchObject({
      operation: "update_packages",
      target_id: "target-101",
      mode: "plan",
      parameters: { operation: "update_packages", packages: ["nginx", "curl"] },
    });
    expect(request).not.toHaveProperty("playbook");
    expect(request).not.toHaveProperty("shell");
  });

  it("sends bearer tokens only as authorization headers and rejects viewer mutations locally", async () => {
    const client = new ApiClient();
    client.setCredentials("opaque-test-token", "viewer");
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: [], request_id: "req-auth" }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    await client.get("/api/v1/targets");
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer opaque-test-token");
    await expect(client.post("/api/v1/targets", {})).rejects.toMatchObject({ status: 403, code: "permission_denied" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("exposes typed resource helpers with the expected endpoints and payloads", async () => {
    const get = vi.spyOn(apiClient, "get").mockResolvedValue([] as never);
    const post = vi.spyOn(apiClient, "post").mockResolvedValue({} as never);
    const del = vi.spyOn(apiClient, "delete").mockResolvedValue(undefined as never);
    await listTargets();
    await createTarget({ name: "t", kind: "lxc", address: "127.0.0.1", transport: "ssh", credential_secret_ref: "c", agent_secret_ref: "a" });
    await createAnsibleJob({ operation: "health_check", target_id: "t", mode: "check", parameters: { operation: "health_check" }, idempotency_key: "k", confirmed: true });
    await listAnsibleJobs();
    await getAnsibleJob("job");
    await getAnsibleJobEvents("job");
    await createEnrollment(1, undefined, false, "00000000-0000-0000-0000-000000000001");
    await getEnrollment("enroll");
    await listSecrets();
    await listSecretAudit();
    await createSecret({ name: "s", kind: "generic", scope: { type: "global" }, value: "v" });
    await rotateSecret("s", "v2", false);
    await revokeSecret("s", false);
    await deleteSecret("s", false);
    await createContainerAction(1, "refresh", true);
    await getAgentHealth(1);
    await getAgentMetrics(1);
    await listDockerWorkloads(1);
    await discoverDockerWorkloads(1);
    await adoptDockerWorkload(1, "docker/id");
    await removeDockerWorkload(1, "docker/id");
    expect(get).toHaveBeenCalled();
    expect(post).toHaveBeenCalled();
    expect(del).toHaveBeenCalled();
    get.mockRestore();
    post.mockRestore();
    del.mockRestore();
  });

  it("authenticates SSE streams, parses frames and closes subscriptions", async () => {
    const encoder = new TextEncoder();
    const stream = new ReadableStream<Uint8Array>({ start(controller) {
      controller.enqueue(encoder.encode('event: status\ndata: {"type":"Status","payload":{"resource":"target","resource_id":"t","state":"managed"}}\n\nevent: log\ndata: not-json\n\n'));
      controller.close();
    } });
    const fetchMock = vi.fn().mockResolvedValue(new Response(stream, { headers: { "content-type": "text/event-stream" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient();
    client.setCredentials("event-token", "admin");
    const onEvent = vi.fn();
    const onError = vi.fn();
    const unsubscribe = client.subscribe(onEvent, onError);
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledTimes(1));
    unsubscribe();
    expect(onEvent).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(new Headers(fetchMock.mock.calls[0]?.[1]?.headers).get("authorization")).toBe("Bearer event-token");
    expect(String(fetchMock.mock.calls[0]?.[0])).not.toContain("event-token");
  });
});
