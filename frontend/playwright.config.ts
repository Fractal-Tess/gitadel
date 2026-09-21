import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.e2e.ts",
  fullyParallel: false,
  reporter: "line",
  use: {
    baseURL: process.env.GITADEL_E2E_BASE_URL ?? "http://127.0.0.1:3137",
    trace: "retain-on-failure",
  },
});
