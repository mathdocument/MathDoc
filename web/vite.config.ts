import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// In `vite dev` mode the backend runs on a random port (127.0.0.1:NNNN).
// During development we proxy /api → that backend; the dev port is set via
// the MDC_API_PROXY env var (defaults to 127.0.0.1:0 fallback handled below).
const apiTarget = process.env.MDC_API_PROXY ?? "http://127.0.0.1:7599";

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    strictPort: false,
    proxy: {
      "/api": {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
  },
});
