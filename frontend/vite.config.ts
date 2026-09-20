import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "VITE_");

  return {
    plugins: [react()],
    server: {
      host: "0.0.0.0",
      port: 5173,
      proxy: {
        "/api": env.VITE_API_PROXY_TARGET ?? "http://127.0.0.1:8080",
      },
    },
    test: {
      environment: "jsdom",
      setupFiles: "./src/test-setup.ts",
      coverage: {
        provider: "v8",
        reporter: ["text", "lcov"],
        reportsDirectory: "./coverage",
        exclude: [
          "src/test-setup.ts",
          "src/**/*.test.ts",
          "src/**/*.test.tsx",
          "src/**/*.d.ts",
          "vite.config.ts",
          "dist/**",
          "coverage/**",
        ],
      },
    },
  };
});
