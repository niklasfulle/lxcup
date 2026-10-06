import { describe, expect, it, vi } from "vitest";
import {
  ApiClient,
  ApiError,
  adoptDockerWorkload,
  createAnsibleJob,
  createContainerAction,
  createEnrollment,
  createAdminBackup,
  createSecret,
  createTarget,
  deleteSecret,
  deleteUpdatePolicy,
  discoverDockerWorkloads,
  getAgentHealth,
  getAgentMetrics,
  getAnsibleJob,
  getAnsibleJobEvents,
  getEnrollment,
  getTargetDockerInventory,
  listAnsibleJobs,
  listDockerWorkloads,
  listSecretAudit,
  listUserAudit,
  listSecrets,
  listTargets,
  removeDockerWorkload,
  reconcileAnsibleJob,
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

  it("does not retry non-idempotent requests after a transient server failure", async () => {
    vi.stubGlobal("fetch", vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: { code: "internal_error", message: "temporary" }, request_id: "req-post-1" }), { status: 503 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { id: "created" }, request_id: "req-post-2" }), { status: 201 })));

    await expect(new ApiClient().post("/api/v1/targets", { name: "duplicate-label" }))
      .rejects.toEqual(expect.objectContaining({ status: 503, code: "internal_error" }));
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("rejects successful responses without the versioned data envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("not-json", { status: 200 })));

    await expect(new ApiClient().get("/api/v1/targets")).rejects.toEqual(expect.objectContaining({ code: "invalid_response", status: 200 }));
  });

  it("builds only allowlisted workflow fields without playbook or shell input", () => {
    const request = buildWorkflowRequest({ targetId: "target-101", operation: "update_packages", mode: "plan", packages: "nginx, curl", confirmed: false });

    expect(request).toMatchObject({
      operation: "update_packages",
      target_id: "target-101",
      mode: "plan",
      parameters: { operation: "update_packages", packages: ["nginx", "curl"] },
    });
    expect(request).not.toHaveProperty("playbook");
    expect(request).not.toHaveProperty("shell");
  });

  it("builds a read-only package inventory workflow request", () => {
    const request = buildWorkflowRequest({ targetId: "target-inventory", operation: "collect_package_inventory", mode: "check", packages: "", confirmed: false });

    expect(request).toMatchObject({
      operation: "collect_package_inventory",
      target_id: "target-inventory",
      mode: "check",
      parameters: { operation: "collect_package_inventory" },
      confirmed: false,
    });
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

  it("streams encrypted admin backups and blocks non-admin downloads locally", async () => {
    const backupBlob = new Blob(["ciphertext"]);
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 200, blob: async () => backupBlob } as Response);
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient();
    client.setCredentials("admin-token", "admin");
    const backup = await client.getBlob("/api/v1/admin/backups/backup-1");
    expect(backup).toBe(backupBlob);
    expect(new Headers(fetchMock.mock.calls[0]?.[1]?.headers).get("accept")).toBe("application/octet-stream");

    client.setCredentials("user-token", "user");
    await expect(client.getBlob("/api/v1/admin/backups/backup-1")).rejects.toMatchObject({ status: 403, code: "permission_denied" });
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("sends the one-time backup passphrase only with the create request", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: { id: "backup-1", created_at: "2026-10-02T10:00:00Z", size_bytes: 2048 }, request_id: "req-backup" }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    apiClient.setCredentials("admin-token", "admin");

    await createAdminBackup("correct horse battery staple");

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(JSON.parse(String(init.body))).toEqual({ confirmed: true, passphrase: "correct horse battery staple" });
    expect(String(init.headers)).not.toContain("correct horse battery staple");
    apiClient.setCredentials(null, null);
  });

  it("sends same-origin cookies and mirrors the CSRF cookie on state-changing requests", async () => {
    document.cookie = "lxcup_csrf=test-csrf-token; Path=/";
    const client = new ApiClient();
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await client.logout();

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.credentials).toBe("same-origin");
    expect(new Headers(init.headers).get("x-csrf-token")).toBe("test-csrf-token");
    document.cookie = "lxcup_csrf=; Max-Age=0; Path=/";
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
    await reconcileAnsibleJob("job-reconcile");
    await createEnrollment(1, undefined, false, "00000000-0000-0000-0000-000000000001");
    await getEnrollment("enroll");
    await listSecrets();
    await listSecretAudit();
    await createSecret({ name: "s", kind: "generic", scope: { type: "global" }, value: "v" });
    await rotateSecret("s", "v2", false);
    await revokeSecret("s", false);
    await deleteSecret("s", false);
    await deleteUpdatePolicy("legacy-policy", true);
    await createContainerAction(1, "refresh", true);
    await getAgentHealth(1);
    await getAgentMetrics(1);
    await listDockerWorkloads(1);
    await getTargetDockerInventory("target/id");
    await discoverDockerWorkloads(1);
    await adoptDockerWorkload(1, "docker/id");
    await removeDockerWorkload(1, "docker/id");
    expect(get).toHaveBeenCalled();
    expect(get).toHaveBeenCalledWith("/api/v1/targets/target%2Fid/docker/discovery", undefined);
    expect(post).toHaveBeenCalled();
    expect(post).toHaveBeenCalledWith("/api/v1/ansible/jobs/job-reconcile/reconcile", {}, undefined);
    expect(del).toHaveBeenCalledWith("/api/v1/update-policies/legacy-policy", { confirmed: true }, undefined);
    expect(del).toHaveBeenCalled();
    get.mockRestore();
    post.mockRestore();
    del.mockRestore();
  });

  it("builds encoded audit filter and pagination parameters", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: { events: [], total: 0, limit: 50, offset: 0 }, request_id: "req-audit" }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    await listUserAudit({ limit: 50, offset: 0, actor_username: "admin user", action: "user.created", resource: "target-1", since: "2026-09-01T00:00:00.000Z" });
    const requestUrl = new URL(String(fetchMock.mock.calls[0]?.[0]), "http://localhost");
    expect(requestUrl.pathname).toBe("/api/v1/auth/audit");
    expect(requestUrl.searchParams.get("actor_username")).toBe("admin user");
    expect(requestUrl.searchParams.get("action")).toBe("user.created");
    expect(requestUrl.searchParams.get("resource")).toBe("target-1");
    expect(requestUrl.searchParams.get("since")).toBe("2026-09-01T00:00:00.000Z");
    expect(requestUrl.searchParams.get("offset")).toBe("0");
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
