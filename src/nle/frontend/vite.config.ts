import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// `devUrl` in ../tauri.conf.json names the same port. ../scripts/dev.ts passes
// a port from portplz on the command line, and that port wins over this one.
const DEFAULT_DEV_PORT = 1420;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: DEFAULT_DEV_PORT,
    strictPort: true,
  },
  // TAURI_ENV_ and not TAURI_: the Tauri CLI reads the updater signing key
  // from TAURI_SIGNING_PRIVATE_KEY, and the frontend must never get it.
  envPrefix: ["VITE_", "TAURI_ENV_"],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
