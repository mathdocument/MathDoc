import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { gzipSync } from "node:zlib";
import { cpus, platform, arch, release } from "node:os";
import { once } from "node:events";
import { readFile, readdir, stat, writeFile } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import {
  apiBodies,
  EDITOR_LINE_COUNT,
  GRAPH_EDGE_COUNT,
  GRAPH_NODE_COUNT,
  RELATION_COUNT,
} from "./fixtures.mjs";

const perfDir = dirname(fileURLToPath(import.meta.url));
const defaultRoot = resolve(perfDir, "..");
const args = process.argv.slice(2);

function option(name) {
  const exact = args.indexOf(`--${name}`);
  if (exact !== -1) return args[exact + 1];
  const prefix = `--${name}=`;
  return args.find((arg) => arg.startsWith(prefix))?.slice(prefix.length);
}

const webRoot = resolve(option("root") ?? defaultRoot);
const record = args.includes("--record");
const outputPath = resolve(
  option("output") ?? resolve(perfDir, record ? "baseline.json" : "latest.json"),
);
const comparePath = option("compare")
  ? resolve(option("compare"))
  : !record && !option("output")
    ? resolve(perfDir, "baseline.json")
    : null;
const sampleCount = Number(process.env.MDC_PERF_SAMPLES ?? 5);

if (!Number.isInteger(sampleCount) || sampleCount < 2) {
  throw new Error("MDC_PERF_SAMPLES must be an integer of at least 2");
}

function round(value) {
  return Math.round(value * 100) / 100;
}

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

async function filesUnder(path) {
  const entries = await readdir(path, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const entryPath = resolve(path, entry.name);
    return entry.isDirectory() ? filesUnder(entryPath) : [entryPath];
  }));
  return nested.flat();
}

function transferSize(path, contents) {
  return [".css", ".html", ".js", ".json", ".svg"].includes(extname(path))
    ? gzipSync(contents, { level: 9 }).length
    : contents.length;
}

async function bundleMetrics() {
  const dist = resolve(webRoot, "dist");
  const indexPath = resolve(dist, "index.html");
  const index = await readFile(indexPath, "utf8");
  const initialPaths = new Set([indexPath]);
  for (const match of index.matchAll(/(?:href|src)="(\/[^"?#]+)"/g)) {
    const path = resolve(dist, `.${match[1]}`);
    try {
      if ((await stat(path)).isFile()) initialPaths.add(path);
    } catch {
      // External or development-only path; Vite production output should not need it.
    }
  }
  const allPaths = await filesUnder(dist);
  const sizes = await Promise.all(allPaths.map(async (path) => transferSize(path, await readFile(path))));
  const initialSizes = await Promise.all(
    [...initialPaths].map(async (path) => transferSize(path, await readFile(path))),
  );
  return {
    shellTransferBytes: initialSizes.reduce((sum, size) => sum + size, 0),
    totalTransferBytes: sizes.reduce((sum, size) => sum + size, 0),
  };
}

async function availablePort() {
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  server.close();
  await once(server, "close");
  return port;
}

async function startPreview() {
  const port = await availablePort();
  const vite = resolve(webRoot, "node_modules/vite/bin/vite.js");
  const child = spawn(process.execPath, [
    vite,
    "preview",
    "--host", "127.0.0.1",
    "--port", String(port),
    "--strictPort",
  ], { cwd: webRoot, stdio: ["ignore", "pipe", "pipe"] });
  let logs = "";
  child.stdout.on("data", (chunk) => { logs += chunk; });
  child.stderr.on("data", (chunk) => { logs += chunk; });
  const url = `http://127.0.0.1:${port}`;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw new Error(`Vite preview exited early:\n${logs}`);
    try {
      const response = await fetch(url);
      if (response.ok) return { child, url };
    } catch {
      // The preview process is still starting.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
  }
  child.kill();
  throw new Error(`Timed out starting Vite preview:\n${logs}`);
}

async function stopPreview(child) {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    once(child, "exit"),
    new Promise((resolveDelay) => setTimeout(resolveDelay, 2_000)),
  ]);
  if (child.exitCode === null) child.kill("SIGKILL");
}

async function preparePage(context, scenario) {
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    localStorage.setItem("mdc-theme", "dark");
    window.__mdcPerfStart = performance.now();
  });
  const bodies = apiBodies(scenario);
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/p\/benchmark\/main/, "");
    const body = bodies.get(path);
    await route.fulfill(body === undefined
      ? { status: 404, contentType: "application/json", body: '{"error":"missing perf fixture"}' }
      : { status: 200, contentType: "application/json", body });
  });
  return { page, errors };
}

async function nextPaint(page) {
  await page.evaluate(() => new Promise((resolvePaint) => {
    requestAnimationFrame(() => requestAnimationFrame(resolvePaint));
  }));
}

async function runEditorSample(context, url) {
  const { page, errors } = await preparePage(context, "editor");
  try {
    await page.goto(`${url}/p/benchmark/main/`, { waitUntil: "domcontentloaded" });
    await page.locator('.cm-editor').waitFor({ state: "visible" });
    await nextPaint(page);
    const editorReadyMs = await page.evaluate(() => performance.now() - window.__mdcPerfStart);

    const highlighted = page.locator('.cm-content span[style*="color:"]').first();
    await highlighted.waitFor({ state: "visible" });
    const editorHighlightMs = await page.evaluate(() => performance.now() - window.__mdcPerfStart);
    const darkStyle = await highlighted.getAttribute("style");

    await page.evaluate(() => { window.__mdcPerfAction = performance.now(); });
    await page.getByTitle("Switch to light mode").click();
    await page.waitForFunction((previousStyle) => {
      const token = document.querySelector('.cm-content span[style*="color:"]');
      return document.documentElement.dataset.theme === "light" &&
        token?.getAttribute("style") !== previousStyle;
    }, darkStyle);
    await nextPaint(page);
    const themeSwitchMs = await page.evaluate(() => performance.now() - window.__mdcPerfAction);

    await page.evaluate(() => { window.__mdcPerfAction = performance.now(); });
    await page.getByTitle("Render LaTeX preview").click();
    await page.locator(".latex-preview .katex").first().waitFor({ state: "visible" });
    await nextPaint(page);
    const latexPreviewMs = await page.evaluate(() => performance.now() - window.__mdcPerfAction);
    const proofText = await page.locator(".latex-statement.proof .latex-statement-body").innerText();
    if (proofText.trim() !== "Inline proof.") throw new Error("inline proof environment did not close");
    await page.setViewportSize({ width: 375, height: 667 });
    const mobileLayout = await page.evaluate(() => ({
      editorWidth: document.querySelector(".editor-wrap > .center")?.getBoundingClientRect().width ?? 0,
      sidebarsVisible: [...document.querySelectorAll(".layout > .column")]
        .some((element) => getComputedStyle(element).display !== "none"),
    }));
    if (mobileLayout.editorWidth < 300 || mobileLayout.sidebarsVisible) {
      throw new Error("mobile editor layout is obstructed by relation columns");
    }
    if (errors.length > 0) throw new Error(errors.join("\n"));
    return { editorReadyMs, editorHighlightMs, themeSwitchMs, latexPreviewMs };
  } finally {
    await page.close();
  }
}

async function runGraphSample(context, url) {
  const { page, errors } = await preparePage(context, "graph");
  try {
    await page.addInitScript(() => {
      window.__canvasResizes = { total: 0, unpainted: 0 };
      const paints = new WeakMap();
      const fill = CanvasRenderingContext2D.prototype.fill;
      CanvasRenderingContext2D.prototype.fill = function (...args) {
        paints.set(this.canvas, (paints.get(this.canvas) ?? 0) + 1);
        return fill.apply(this, args);
      };
      for (const key of ["width", "height"]) {
        const descriptor = Object.getOwnPropertyDescriptor(HTMLCanvasElement.prototype, key);
        Object.defineProperty(HTMLCanvasElement.prototype, key, { ...descriptor, set(value) {
          descriptor.set.call(this, value);
          if (!this.closest(".graph-container") || value <= 1) return;
          const before = paints.get(this);
          window.__canvasResizes.total++;
          queueMicrotask(() => {
            if (this.width > 1 && this.height > 1 && paints.get(this) === before) {
              window.__canvasResizes.unpainted++;
            }
          });
        } });
      }
    });
    await page.goto(`${url}/p/benchmark/main/`, { waitUntil: "domcontentloaded" });
    await page.locator("h1.title", { hasText: "Performance fixture" }).waitFor();
    const graphResponse = page.waitForResponse((response) =>
      new URL(response.url()).pathname === "/p/benchmark/main/api/graph/full",
    );
    await page.evaluate(() => { window.__mdcPerfAction = performance.now(); });
    await page.getByTitle("Graph view").click();
    await graphResponse;
    await page.waitForFunction(() => {
      const canvas = document.querySelector(".force-layout:not(.hidden) canvas");
      return canvas instanceof HTMLCanvasElement && canvas.width > 1 && canvas.height > 1 &&
        document.querySelector(".graph-loading") === null;
    });
    await nextPaint(page);
    const graphError = page.locator(".graph-container [role=alert]");
    if (await graphError.count() > 0) throw new Error(await graphError.innerText());
    const graphReadyMs = await page.evaluate(() => performance.now() - window.__mdcPerfAction);
    const zoomFrames = await page.evaluate(async () => {
      const canvas = document.querySelector(".force-layout:not(.hidden) canvas");
      if (!(canvas instanceof HTMLCanvasElement)) throw new Error("graph canvas missing");
      const frameTimes = [];
      for (let index = 0; index < 20; index++) {
        await new Promise((resolveFrame) => requestAnimationFrame(resolveFrame));
        const rect = canvas.getBoundingClientRect();
        const start = performance.now();
        canvas.dispatchEvent(new WheelEvent("wheel", {
          bubbles: true,
          cancelable: true,
          clientX: rect.left + rect.width / 2,
          clientY: rect.top + rect.height / 2,
          deltaY: index % 2 === 0 ? -40 : 40,
        }));
        await new Promise((resolveFrame) => requestAnimationFrame(resolveFrame));
        frameTimes.push(performance.now() - start);
      }
      return frameTimes;
    });
    const canvasBounds = await page.locator(".force-layout:not(.hidden) canvas").boundingBox();
    if (!canvasBounds) throw new Error("graph canvas is not measurable");
    await page.mouse.move(canvasBounds.x + canvasBounds.width / 2, canvasBounds.y + canvasBounds.height / 2);
    await page.mouse.down();
    await page.mouse.move(canvasBounds.x + canvasBounds.width / 2 + 30, canvasBounds.y + canvasBounds.height / 2 + 20);
    await page.mouse.up();
    await nextPaint(page);
    const divider = page.getByRole("separator", { name: "Resize graph editor" });
    const bounds = await divider.boundingBox();
    const beforeWidth = await page.locator("canvas").getAttribute("width");
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + 100);
    await page.mouse.down();
    await page.mouse.move(bounds.x - 180, bounds.y + 100, { steps: 20 });
    await page.mouse.up();
    await nextPaint(page);
    if (await page.locator("canvas").getAttribute("width") === beforeWidth) {
      throw new Error("sidebar drag did not resize the graph");
    }
    const resizes = await page.evaluate(() => window.__canvasResizes);
    if (resizes.total < 2 || resizes.unpainted !== 0) {
      throw new Error(`canvas resize exposed an empty frame: ${JSON.stringify(resizes)}`);
    }
    await divider.press("ArrowRight");
    const focus = await divider.evaluate(element => ({
      outer: getComputedStyle(element).outlineStyle,
      handle: getComputedStyle(element, "::after").outlineStyle,
    }));
    if (focus.outer !== "none" || focus.handle !== "solid") {
      throw new Error("splitter keyboard focus should outline only the middle handle");
    }
    if (errors.length > 0) throw new Error(errors.join("\n"));
    return { graphReadyMs, zoomFrames };
  } finally {
    await page.close();
  }
}

async function runRelationsSample(context, url) {
  const { page, errors } = await preparePage(context, "relations");
  try {
    let graphRequests = 0;
    page.on("request", request => { if (request.url().endsWith("/api/graph/full")) graphRequests++; });
    await page.goto(`${url}/p/benchmark/main/`);
    const column = page.getByRole("complementary", { name: "Dependencies" });
    const cards = column.locator(".card");
    await cards.first().waitFor();
    await nextPaint(page);
    const readyMs = await page.evaluate(() => performance.now() - window.__mdcPerfStart);
    const renderedCards = await cards.count();
    if (renderedCards > 50 || await column.locator(".count").innerText() !== String(RELATION_COUNT)) {
      throw new Error(`large relation list rendered ${renderedCards} cards instead of a visible window`);
    }
    const list = column.locator(".cards");
    await list.evaluate(element => { element.scrollTop = element.scrollHeight / 2; });
    await nextPaint(page);
    const position = await list.evaluate(element => element.scrollTop);
    const firstIndex = await cards.first().getAttribute("data-index");
    if (Number(firstIndex) < RELATION_COUNT / 3) throw new Error("relation window did not follow scrolling");
    await page.getByTitle("Graph view", { exact: true }).click();
    await page.locator(".graph-container canvas").waitFor();
    await page.waitForFunction(() => !document.querySelector(".graph-loading"));
    await nextPaint(page);
    const switches = [];
    for (let i = 0; i < 4; i++) {
      await page.evaluate(() => window.__mdcPerfAction = performance.now());
      await page.getByTitle(i % 2 ? "Graph view" : "Knowledge view", { exact: true }).click();
      await nextPaint(page);
      switches.push(await page.evaluate(() => performance.now() - window.__mdcPerfAction));
      if (i % 2 === 0 && Math.abs(await list.evaluate(element => element.scrollTop) - position) > 1) {
        throw new Error("view switching lost the relation scroll position");
      }
    }
    if (graphRequests !== 1) throw new Error("view switching reloaded the graph");
    await page.getByTitle("Knowledge view", { exact: true }).click();
    await cards.first().press("End");
    const lastId = `perf-node-${String(RELATION_COUNT).padStart(5, "0")}`;
    await page.waitForFunction(id => document.activeElement?.getAttribute("data-fnode") === id, lastId);
    const last = column.locator(`[data-fnode="${lastId}"]`);
    if (await last.locator("..").getAttribute("aria-posinset") !== String(RELATION_COUNT)) {
      throw new Error("virtual list lost accessible item positions");
    }
    await last.press("Home");
    await page.waitForFunction(() => document.activeElement?.getAttribute("data-index") === "0");
    for (let i = 0; i < 30; i++) await page.keyboard.press("ArrowDown");
    await page.waitForFunction(() => document.activeElement?.getAttribute("data-index") === "30");
    await page.keyboard.press("End");
    await last.click();
    await page.locator("h1.title", { hasText: `Deterministic graph node ${RELATION_COUNT}` }).waitFor();
    await page.getByRole("complementary", { name: "Referrers" }).locator(".card").click();
    await page.locator("h1.title", { hasText: "Performance fixture" }).waitFor();
    if (await list.evaluate(element => element.scrollTop) !== 0) throw new Error("new node retained the old list's scroll offset");
    if (errors.length) throw new Error(errors.join("\n"));
    return { readyMs, renderedCards, switchMs: switches.map(round) };
  } finally {
    await page.close();
  }
}

function metric(value, unit) {
  return { value: round(value), unit };
}

function compareReports(base, current, budgets) {
  const failures = [];
  const rows = [];
  const comparableEnvironment = ({ os, cpu, node, browser, viewport, samples }) => ({
    os: os.split(" ").slice(0, 2).join(" "),
    cpu,
    node: node.split(".")[0],
    browser: browser.split(".")[0],
    viewport,
    samples,
  });
  if (base.schema !== current.schema) failures.push("report schema differs");
  if (JSON.stringify(base.fixtures) !== JSON.stringify(current.fixtures)) {
    failures.push("fixtures differ");
  }
  if (JSON.stringify(comparableEnvironment(base.environment)) !==
    JSON.stringify(comparableEnvironment(current.environment))) {
    failures.push("benchmark environments differ");
  }
  for (const [name, budget] of Object.entries(budgets.metrics)) {
    const before = base.metrics[name]?.value;
    const after = current.metrics[name]?.value;
    if (before === undefined || after === undefined) {
      failures.push(`${name}: missing metric`);
      continue;
    }
    if (base.metrics[name].unit !== current.metrics[name].unit) {
      failures.push(`${name}: units differ`);
      continue;
    }
    const relativeLimit = before * (1 + budget.maxIncreaseRatio);
    const noiseLimit = before + budget.noise;
    const regressionLimit = Math.max(relativeLimit, noiseLimit);
    const limit = budget.max === undefined ? regressionLimit : Math.min(budget.max, regressionLimit);
    const passed = after <= limit;
    rows.push({ metric: name, base: round(before), current: round(after), limit: round(limit), passed });
    if (!passed) failures.push(`${name}: ${round(after)} > ${round(limit)}`);
  }
  console.table(rows);
  if (failures.length > 0) throw new Error(`Performance regression:\n${failures.join("\n")}`);
}

async function main() {
  const bundle = await bundleMetrics();
  const preview = await startPreview();
  let browser;
  try {
    browser = await chromium.launch({
      channel: "chromium",
      headless: true,
      args: ["--enable-precise-memory-info"],
    });
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: "dark",
      reducedMotion: "reduce",
    });

    await runEditorSample(context, preview.url);
    await runGraphSample(context, preview.url);

    const editorSamples = [];
    const graphSamples = [];
    for (let index = 0; index < sampleCount; index++) {
      editorSamples.push(await runEditorSample(context, preview.url));
      graphSamples.push(await runGraphSample(context, preview.url));
    }
    const relations = await runRelationsSample(context, preview.url);
    await context.close();

    const values = (name) => editorSamples.map((sample) => sample[name]);
    const zoomFrames = graphSamples.flatMap((sample) => sample.zoomFrames);
    const report = {
      schema: 1,
      generatedAt: new Date().toISOString(),
      environment: {
        os: `${platform()} ${arch()} ${release()}`,
        cpu: cpus()[0]?.model ?? "unknown",
        node: process.version,
        browser: browser.version(),
        viewport: "1440x900",
        samples: sampleCount,
      },
      fixtures: {
        editorLines: EDITOR_LINE_COUNT,
        graphNodes: GRAPH_NODE_COUNT,
        graphEdges: GRAPH_EDGE_COUNT,
      },
      metrics: {
        "bundle.shellTransferBytes": metric(bundle.shellTransferBytes, "bytes"),
        "bundle.totalTransferBytes": metric(bundle.totalTransferBytes, "bytes"),
        "runtime.editorReadyMs": metric(median(values("editorReadyMs")), "ms"),
        "runtime.editorHighlightMs": metric(median(values("editorHighlightMs")), "ms"),
        "runtime.themeSwitchMs": metric(median(values("themeSwitchMs")), "ms"),
        "runtime.latexPreviewMs": metric(median(values("latexPreviewMs")), "ms"),
        "runtime.graphReadyMs": metric(
          median(graphSamples.map((sample) => sample.graphReadyMs)),
          "ms",
        ),
        "runtime.graphZoomFrameP95Ms": metric(percentile(zoomFrames, 0.95), "ms"),
      },
      rawSamples: {
        relations,
        editor: editorSamples.map((sample) => Object.fromEntries(
          Object.entries(sample).map(([name, value]) => [name, round(value)]),
        )),
        graphReadyMs: graphSamples.map((sample) => round(sample.graphReadyMs)),
        graphZoomFrameMs: zoomFrames.map(round),
      },
    };

    await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`);
    console.table(Object.entries(report.metrics).map(([name, value]) => ({
      metric: name,
      value: value.value,
      unit: value.unit,
    })));
    console.log(`Report: ${outputPath}`);

    if (comparePath) {
      const [base, budgets] = await Promise.all([
        readFile(comparePath, "utf8").then(JSON.parse),
        readFile(resolve(perfDir, "budgets.json"), "utf8").then(JSON.parse),
      ]);
      compareReports(base, report, budgets);
    }
  } finally {
    await browser?.close();
    await stopPreview(preview.child);
  }
}

await main();
