/// <reference types="vitest" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { devServerPort } from "./src/dev-port";

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
