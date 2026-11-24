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
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
