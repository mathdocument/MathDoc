import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { nodePolyfills } from "vite-plugin-node-polyfills";
import { viteStaticCopy } from "vite-plugin-static-copy";
import importMetaUrlPlugin from "@codingame/esbuild-import-meta-url-plugin";
import fs from "node:fs";
import path from "node:path";
import mathjaxPackage from "mathjax/package.json" with { type: "json" };
import { monacoWheel, monacoWheelDeps } from "./build/monaco-wheel";

// Proxy the project APIs and directory to the shared entry server.
const apiTarget = process.env.MDC_API_PROXY ?? "http://127.0.0.1:17843";
const mathjaxVersion = mathjaxPackage.version;
const sveltePlugins = svelte();
const localNodeModules = path.resolve("node_modules");
const workspaceNodeModules = path.resolve("..", "node_modules");
const nodeModules = fs.existsSync(path.join(localNodeModules, "mathjax"))
  ? localNodeModules
  : workspaceNodeModules;
const dependencyPath = (relativePath: string) => path.join(nodeModules, relativePath);

// svelte-check 4.6 expects the config-only plugin name introduced after plugin-svelte 5.
sveltePlugins.push({ name: "vite-plugin-svelte:config", api: sveltePlugins[0]!.api });

export default defineConfig({
  plugins: [
    monacoWheel,
    {
      name: "project-editor-page",
      transformIndexHtml: {
        order: "post",
        // Vite replaces entry scripts during build and drops blocking="render".
        handler(html, ctx) {
          return ctx.bundle && ctx.path === "/index.html"
            ? html.replace('<script type="module"', '<script blocking="render" type="module"')
            : html;
        },
      },
    },
    nodePolyfills({ overrides: { fs: "memfs" } }),
    viteStaticCopy({ targets: [
      { src: dependencyPath("@drgrice1/tikzjax/dist/{run-tex.js,run-tex.js.map,core.dump.gz,tex.wasm.gz,fonts.css,fonts,tex_files}"), dest: "tikz/1.0.0-beta24" },
      { src: dependencyPath("mathjax/{tex-chtml-nofont.js,LICENSE}"), dest: `mathjax/${mathjaxVersion}` },
      { src: dependencyPath("mathjax/input/tex"), dest: `mathjax/${mathjaxVersion}/input` },
      { src: dependencyPath("mathjax/ui/safe.js"), dest: `mathjax/${mathjaxVersion}/ui` },
      { src: dependencyPath("mathjax/a11y/assistive-mml.js"), dest: `mathjax/${mathjaxVersion}/a11y` },
      { src: dependencyPath("@mathjax/mathjax-tex-font/{chtml.js,chtml}"), dest: `mathjax/${mathjaxVersion}/fonts/mathjax-tex-font` },
    ] }),
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
      "^/p/[^/]+/[^/]+/api(?:/|$)": { target: apiTarget, changeOrigin: false, ws: true },
    },
  },
  optimizeDeps: { esbuildOptions: { plugins: [monacoWheelDeps, importMetaUrlPlugin] } },
  build: { rollupOptions: { input: { main: path.resolve("index.html") } } },
});
