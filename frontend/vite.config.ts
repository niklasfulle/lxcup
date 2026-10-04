import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const staticClientRoutes = new Set([
  "/",
  "/servers",
  "/windows",
  "/containers",
  "/targets",
  "/docker",
  "/enrollments/new",
  "/workflows",
  "/schedules",
  "/update-policies",
  "/secrets",
  "/account/password",
  "/admin/users",
  "/admin/audit",
  "/wiki",
]);

export function isKnownClientRoute(pathname: string) {
  const normalizedPath = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return staticClientRoutes.has(normalizedPath)
    || /^\/targets\/[^/]+(?:\/packages)?$/.test(normalizedPath)
    || /^\/containers\/[^/]+$/.test(normalizedPath)
    || /^\/workflows\/[^/]+$/.test(normalizedPath);
}

export function frontendDocumentStatusPlugin(): Plugin {
  return {
    name: "lxcup-frontend-document-status",
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        markUnknownDocument(request as unknown as FrontendRequest, response);
        next();
      });
    },
    configurePreviewServer(server) {
      server.middlewares.use((request, response, next) => {
        markUnknownDocument(request as unknown as FrontendRequest, response);
        next();
      });
    },
  };
}

type FrontendRequest = { headers: { accept?: string }; method?: string; url?: string };

function markUnknownDocument(
  request: FrontendRequest,
  response: { statusCode: number },
) {
  const acceptsHtml = request.headers.accept?.includes("text/html") ?? false;
  const isDocumentRequest = request.method === "GET" || request.method === "HEAD";
  const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
  if (acceptsHtml && isDocumentRequest && !pathname.startsWith("/api/") && pathname !== "/api" && !isKnownClientRoute(pathname)) {
    response.statusCode = 404;
    const writableResponse = response as unknown as { writeHead: (...args: unknown[]) => unknown };
    const originalWriteHead = writableResponse.writeHead;
    writableResponse.writeHead = function (...args: unknown[]) {
      if (args[0] === 200) args[0] = 404;
      return originalWriteHead.apply(writableResponse, args);
    };
  }
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "VITE_");

  return {
    plugins: [react(), frontendDocumentStatusPlugin()],
    server: {
      host: "0.0.0.0",
      port: 5173,
      proxy: {
        "/api": env.VITE_API_PROXY_TARGET ?? "http://127.0.0.1:8080",
        "/agent": env.VITE_AGENT_ARTIFACTS_PROXY_TARGET ?? "http://127.0.0.1:8081",
      },
    },
    test: {
      environment: "jsdom",
      setupFiles: "./src/test-setup.ts",
      coverage: {
        provider: "v8",
        reporter: ["text", "lcov"],
        reportsDirectory: "./coverage",
        thresholds: {
          lines: 80,
          functions: 80,
          branches: 80,
          statements: 80,
        },
        exclude: [
          "src/test-setup.ts",
          "src/main.tsx",
          "src/tests.rs",
          "src/**/*.test.ts",
          "src/**/*.test.tsx",
          "src/**/*.d.ts",
          "vite.config.ts",
          "**/*.config.*",
          "postcss.config.cjs",
          "tailwind.config.ts",
          "dist/**",
          "coverage/**",
        ],
      },
    },
  };
});
