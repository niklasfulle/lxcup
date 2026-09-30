import type { ViteDevServer } from "vite";
import { describe, expect, it, vi } from "vitest";
import { frontendDocumentStatusPlugin, isKnownClientRoute } from "./vite.config";

type RequestLike = { headers: { accept?: string }; method?: string; url?: string };
type ResponseLike = { statusCode: number; writeHead: (statusCode: number) => unknown };
type Next = () => void;
type Middleware = (request: RequestLike, response: ResponseLike, next: Next) => void;

function documentStatusMiddleware() {
  let middleware: Middleware | undefined;
  const use = vi.fn((nextMiddleware: Middleware) => { middleware = nextMiddleware; });
  const server = { middlewares: { use } } as unknown as ViteDevServer;
  const hook = frontendDocumentStatusPlugin().configureServer;
  if (typeof hook !== "function") throw new Error("Vite server hook was not registered");
  hook(server);
  if (!middleware) throw new Error("Document status middleware was not registered");
  return middleware;
}

describe("frontend document status", () => {
  it("recognizes registered static and parameterized client routes", () => {
    expect(isKnownClientRoute("/" )).toBe(true);
    expect(isKnownClientRoute("/admin/users/")).toBe(true);
    expect(isKnownClientRoute("/targets/target-1/packages")).toBe(true);
    expect(isKnownClientRoute("/workflows/job-1")).toBe(true);
    expect(isKnownClientRoute("/missing-route")).toBe(false);
  });

  it("sets HTTP 404 only for unknown HTML navigation and leaves API requests to the proxy", () => {
    const middleware = documentStatusMiddleware();
    const invoke = (url: string, accept = "text/html", method = "GET") => {
      const response: ResponseLike = { statusCode: 200, writeHead(statusCode) { this.statusCode = statusCode; return this; } };
      const next = vi.fn(() => { response.statusCode = 200; response.writeHead(200); });
      middleware({ url, method, headers: { accept } }, response, next);
      expect(next).toHaveBeenCalledOnce();
      return response.statusCode;
    };

    expect(invoke("/missing-route?from=direct-link")).toBe(404);
    expect(invoke("/admin/users")).toBe(200);
    expect(invoke("/api/v1/unknown")).toBe(200);
    expect(invoke("/missing.js", "*/*")).toBe(200);
    expect(invoke("/missing-route", "text/html", "POST")).toBe(200);
  });
});
