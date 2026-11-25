import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// The port comes from the `--port` flag of `beforeDevCommand`. `strictPort`
// makes Vite fail instead of moving to another port.
export default defineConfig({
  clearScreen: false,
  build: {
    // The repository guards walk the tree and skip only target and
    // node_modules, thus the minified bundle goes under target.
    // src/msgs/tauri.conf.json (frontendDist) names the same directory.
    outDir: "../../../target/msgs-frontend",
    emptyOutDir: true,
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host } : undefined,
  },
});
