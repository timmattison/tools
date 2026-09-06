import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { devServerPort } from "./dev-port";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: devServerPort(),
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
