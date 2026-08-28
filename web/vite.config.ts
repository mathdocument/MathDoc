import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// In `vite dev` mode the backend runs on a random port (127.0.0.1:NNNN).
// During development we proxy /api → that backend; the dev port is set via
// the MDC_API_PROXY env var (defaults to 127.0.0.1:0 fallback handled below).
const apiTarget = process.env.MDC_API_PROXY ?? "http://127.0.0.1:7599";
const sveltePlugins = svelte();

// svelte-check 4.6 expects the config-only plugin name introduced after plugin-svelte 5.
sveltePlugins.push({ name: "vite-plugin-svelte:config", api: sveltePlugins[0]!.api });

export default defineConfig({
  plugins: [
    {
      name: "katex-woff2-only",
      enforce: "pre",
      transform(code, id) {
        if (!id.endsWith("/katex/dist/katex.min.css")) return;
        return code.replace(
          /,url\(fonts\/[^)]+\.woff\) format\("woff"\),url\(fonts\/[^)]+\.ttf\) format\("truetype"\)/g,
          "",
        );
      },
    },
    ...sveltePlugins,
  ],
  server: {
    proxy: {
      "/api": {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  build: {
    // The oniguruma WASM is inlined as base64 (~620 kB) but only ever loads
    // on demand (dynamic import in src/lib/shiki.ts), so it is not worth
    // flagging as a size regression.
    chunkSizeWarningLimit: 700,
  },
});
