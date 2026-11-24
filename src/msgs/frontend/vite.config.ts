import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// The port comes from the `--port` flag of `beforeDevCommand`. `strictPort`
// makes Vite fail instead of moving to another port.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host } : undefined,
  },
});
