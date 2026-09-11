import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { nodePolyfills } from "vite-plugin-node-polyfills";
import { viteStaticCopy } from "vite-plugin-static-copy";
import importMetaUrlPlugin from "@codingame/esbuild-import-meta-url-plugin";
import path from "node:path";

// Proxy /api to an existing branch service; MDC_API_PROXY can select its allocated port.
const apiTarget = process.env.MDC_API_PROXY ?? "http://127.0.0.1:7599";
const sveltePlugins = svelte();

// svelte-check 4.6 expects the config-only plugin name introduced after plugin-svelte 5.
sveltePlugins.push({ name: "vite-plugin-svelte:config", api: sveltePlugins[0]!.api });

export default defineConfig({
  plugins: [
    nodePolyfills({ overrides: { fs: "memfs" } }),
    viteStaticCopy({ targets: [
      { src: "node_modules/@leanprover/infoview/dist/*", dest: "infoview" },
      { src: "node_modules/lean4monaco/dist/webview/webview.js", dest: "infoview" },
      { src: "node_modules/@leanprover/infoview/dist/codicon.ttf", dest: "assets" },
    ] }),
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
        // Preserve Host alongside Origin for the backend's same-origin checks.
        changeOrigin: false,
        ws: true,
      },
    },
  },
  optimizeDeps: { esbuildOptions: { plugins: [importMetaUrlPlugin] } },
  build: { rollupOptions: { input: { main: path.resolve("index.html"), lean: path.resolve("lean.html") } } },
});
