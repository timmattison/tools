import { defineConfig } from "vitest/config";

// The frontend tests run in happy-dom, a DOM without a browser. Each test
// mocks `@tauri-apps/api/core`, so no test reaches the Rust backend.
export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts", "../scripts/**/*.test.ts"],
    restoreMocks: true,
  },
});
