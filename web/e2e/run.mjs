import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { chromium } from "playwright";
import { createServer } from "node:net";

const run = promisify(execFile);
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = resolve(process.env.MDC_BIN ?? resolve(webRoot, "../target/debug/mdc"));

const env = { ...process.env };
async function startServer(cwd, database) {
  const reservation = createServer();
  await new Promise(resolve => reservation.listen(0, "127.0.0.1", resolve));
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const { stdout } = await run(binary, ["start", `${database}/main`, "--port", String(port)], {
    cwd, env: { ...env, MDC_CACHE_DIR: resolve(cwd, "cache") }, timeout: 30000,
  });
  const info = JSON.parse(stdout);
  return { ...info, url: info.url.replace(/\/$/, ""), output: () => readFileSync(info.log, "utf8").slice(-16000) };
}

async function fixture(browser, body, extraNodes = [], expectedPageErrors = []) {
  const root = await mkdtemp(resolve(tmpdir(), "mdc-e2e-"));
  let server;
  let context;
  const database = `mdce2e${randomUUID().replaceAll("-", "")}`;
  const cli = (...args) => run(binary, (args[0] === "init" ? ["init", database] : [args[0], "--proj", `${database}/main`, ...args.slice(1)]), { cwd: root, env: { ...env, MDC_CACHE_DIR: resolve(root, "cache") }, timeout: 30000 });
  try {
    await cli("init");
    server = await startServer(root, database);
    const bundle = JSON.parse((await cli("export")).stdout);
    bundle.nodes = ["Alpha", "Beta", "Gamma"].map(title => {
      const fnode = randomUUID();
      return { fnode, title, module: `Lib.N_${fnode.replaceAll("-", "")}`, depens: [], blocks: [] };
    }).concat(extraNodes);
    const input = resolve(root, "graph.json");
    await writeFile(input, JSON.stringify(bundle));
    await cli("import", input);
    await cli("dep", "add", "Alpha", "--target", "Beta");
    await cli("graph", "check");
    context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.tracing.start({ screenshots: true, snapshots: true });
    const page = await context.newPage();
    page.setDefaultTimeout(30000);
    const errors = [], traffic = [];
    page.on("websocket", ws => {
      const record = (direction, payload) => {
        traffic.push({ time: Date.now(), direction, message: String(payload).slice(0, 16000) });
        if (traffic.length > 300) traffic.shift();
      };
      ws.on("framesent", ({ payload }) => record("client", payload));
      ws.on("framereceived", ({ payload }) => record("server", payload));
      ws.on("close", () => record("close", ws.url()));
    });
    page.on("console", message => { if (message.type() === "error") console.error("Browser console:", message.text()); });
    page.on("pageerror", (error) => errors.push(error.stack ?? error.message));
    page.on("dialog", (dialog) => void dialog.accept());
    try {
      await page.goto(server.url);
      await title(page, "Alpha");
      await body({ root, cli, page, url: server.url, serverOutput: server.output });
      await cli("graph", "check");
      assert.deepEqual(errors.filter(error => !expectedPageErrors.some(pattern => pattern.test(error))), []);
    } catch (e) {
      const artifacts = process.env.MDC_E2E_ARTIFACTS
        ? resolve(process.env.MDC_E2E_ARTIFACTS, database)
        : await mkdtemp(resolve(tmpdir(), "mdc-e2e-failure-"));
      await mkdir(artifacts, { recursive: true });
      await page.screenshot({ path: resolve(artifacts, "failure.png"), fullPage: true }).catch(console.error);
      await context.tracing.stop({ path: resolve(artifacts, "trace.zip") }).catch(console.error);
      const frames = await Promise.all(page.frames().map(async frame => ({
        url: frame.url(),
        text: await frame.locator("body").innerText({ timeout: 1000 }).catch(() => ""),
        editor: await frame.locator(".view-line, .squiggly-error").evaluateAll(elements => elements.map(el => ({
          text: el.textContent, class: el.className, bounds: el.getBoundingClientRect().toJSON(),
        }))).catch(() => []),
      })));
      await writeFile(resolve(artifacts, "diagnostics.json"), JSON.stringify({ errors, frames, traffic }, null, 2));
      await writeFile(resolve(artifacts, "service.log"), server.output());
      console.error("Browser failure artifacts:", artifacts);
      console.error("Server:", server.output());
      console.error("Browser errors:", errors);
      console.error("Page:", frames.map(frame => frame.text.slice(-3000)));
      throw e;
    }
  } finally {
    await context?.close();
    if (server) {
      await run(binary, ["stop", `${database}/main`], {
        cwd: root, env: { ...env, MDC_CACHE_DIR: resolve(root, "cache") }, timeout: 35000,
      });
      await run(binary, ["stop"], {
        cwd: root, env: { ...env, MDC_CACHE_DIR: resolve(root, "cache") }, timeout: 35000,
      });
    }
    const serverOutput = server?.output() ?? "";
    if (env.MDC_TERMINUS_PASSWORD) {
      const response = await fetch(`${env.MDC_TERMINUS_URL ?? "http://127.0.0.1:6363"}/api/db/admin/${database}`, {
        method: "DELETE",
        headers: { Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? "admin"}:${env.MDC_TERMINUS_PASSWORD}`).toString("base64")}` },
      });
      assert.ok(response.ok || response.status === 404, `test database cleanup failed: ${response.status}`);
    }
    await rm(root, { recursive: true, force: true });
    assert.doesNotMatch(serverOutput, /broken pipe|recursion limit exceeded/, "service shutdown must finish native cleanup before closing the runtime");
  }
}

const center = (page) => page.getByRole("region", { name: "current node" });
async function title(page, name) {
  await center(page).getByRole("button", { name, exact: true }).waitFor();
}
async function rename(page, value) {
  await center(page).getByTitle("Click to rename").click();
  await page.getByRole("textbox", { name: "Node title" }).fill(value);
}
const beta = (page) => page.getByRole("complementary", { name: "Dependencies" })
  .getByRole("button", { name: /^Beta \(/ });

await test("browser with the real MathDoc backend", { timeout: 240000 }, async (suite) => {
  const browser = await chromium.launch({ headless: true, channel: "chromium" });
  try {
    await suite.test("external edits reject a stale save and preserve the browser draft", () =>
      fixture(browser, async ({ cli, page }) => {
        await rename(page, "Browser Alpha");
        await cli("rename", "Alpha", "External Alpha");
        const response = page.waitForResponse((response) => response.url().endsWith("/title") && response.request().method() === "PUT");
        await page.getByRole("button", { name: "Save title", exact: true }).click();
        assert.equal((await response).status(), 412);
        await page.getByText("node changed; reload and retry", { exact: true }).waitFor();
        assert.equal(await page.getByRole("textbox", { name: "Node title" }).inputValue(), "Browser Alpha");
        assert.equal(JSON.parse((await cli("show", "External Alpha")).stdout).title, "External Alpha");
      }));

    await suite.test("navigation waits for an in-flight save before switching nodes", () =>
      fixture(browser, async ({ cli, page }) => {
        let release;
        let entered;
        const gate = new Promise((resolveGate) => { release = resolveGate; });
        const started = new Promise((resolveStarted) => { entered = resolveStarted; });
        await page.route("**/api/node/*/title", async (route) => {
          entered();
          await gate;
          await route.continue(); // Delay delivery; the real server still performs the mutation.
        });
        await rename(page, "Saved Alpha");
        await page.getByRole("button", { name: "Save title", exact: true }).click();
        await started;
        try {
          await beta(page).click();
          assert.equal(await page.getByRole("textbox", { name: "Node title" }).inputValue(), "Saved Alpha");
        } finally { release(); }
        await title(page, "Beta");
        assert.equal(JSON.parse((await cli("show", "Saved Alpha")).stdout).title, "Saved Alpha");
        assert.equal(JSON.parse((await cli("show", "Beta")).stdout).title, "Beta");
      }));

    await suite.test("browser back and forward restore the focused node", () =>
      fixture(browser, async ({ page }) => {
        await beta(page).click();
        await title(page, "Beta");
        await page.goBack();
        await title(page, "Alpha");
        await page.goForward();
        await title(page, "Beta");
        const state = await page.evaluate(() => window.history.state);
        assert.equal(state.entries[state.index], state.fnode);
      }));

    await suite.test("CLI and browser cannot concurrently introduce opposite edges", () =>
      fixture(browser, async ({ cli, page, url }) => {
        const resolveNode = async (ref) => (await (await fetch(`${url}/api/resolve?ref=${ref}`)).json()).fnode;
        const b = await resolveNode("Beta");
        const c = await resolveNode("Gamma");
        const view = await (await fetch(`${url}/api/node/${c}/view`)).json();
        const results = await Promise.all([
          cli("dep", "add", "Beta", "--target", "Gamma").then(() => true, () => false),
          page.evaluate(async ({ b, c, revision }) => (await fetch(`${location.pathname.replace(/\/$/, "")}/api/node/${c}/dep/add`, {
            method: "POST", headers: { "content-type": "application/json", "if-match": `"${revision}"` },
            body: JSON.stringify({ dep_fnode: b }),
          })).ok, { b, c, revision: view.node.revision }),
        ]);
        assert.equal(results.filter(Boolean).length, 1);
        const refreshed = page.waitForResponse((response) => response.url().endsWith("/graph/check"));
        await page.getByRole("button", { name: "Refresh database view" }).click();
        const report = await (await refreshed).json();
        assert.deepEqual(report.cycles, []);
        assert.equal(report.edges, 2);
      }));
    await suite.test("project directory tracks branch services without disconnecting other Lean editors", () => {
      const node = { fnode: randomUUID(), title: "Directory proof", module: "Lib.Directory", depens: [], blocks: [{ srctype: "lean", content: "theorem directoryProof : True := by trivial\n" }] };
      return fixture(browser, async ({ root, cli, page, url }) => {
        const base = new URL(url).origin;
        const project = new URL(url).pathname.slice(3);
        const database = project.split("/")[0];
        const agent = `${database}/agent`;
        const manage = (...args) => run(binary, args, { cwd: root, env: { ...env, MDC_CACHE_DIR: resolve(root, "cache") }, timeout: 35000 });
        await cli("branch", "new", "agent");
        let agentRunning = false;
        try {
          await page.goto(base);
          await page.getByRole("heading", { name: "Projects", exact: true }).waitFor();
          await page.getByRole("textbox", { name: "Search projects and branches" }).fill(database);
          const mainRow = page.locator(`[data-project="${project}"]`);
          const agentRow = page.locator(`[data-project="${agent}"]`);
          await mainRow.getByText("Running", { exact: true }).waitFor();
          await agentRow.getByText("Stopped", { exact: true }).waitFor();
          assert.equal(await agentRow.getByRole("link").count(), 0);
          const started = JSON.parse((await manage("start", agent)).stdout); agentRunning = true;
          assert.equal(new URL(started.url).origin, base);
          await agentRow.getByText("Running", { exact: true }).waitFor({ timeout: 10000 });
          await mainRow.getByRole("link", { name: `Open ${project}` }).click();
          await title(page, "Alpha");
          assert.equal(new URL(page.url()).pathname, `/p/${project}/`);
          let opened = 0, closed = 0;
          page.on("websocket", socket => { opened++; socket.on("close", () => closed++); });
          await page.getByRole("button", { name: /Search nodes/ }).click();
          await page.getByPlaceholder("Search by title or fnode…").fill(node.title);
          await page.getByRole("dialog").getByRole("button", { name: new RegExp(node.title) }).click();
          await page.getByText("Lean editor ready", { exact: true }).waitFor();
          const otherPage = await page.context().newPage();
          let otherClosed = false;
          otherPage.on("websocket", socket => socket.on("close", () => otherClosed = true));
          await otherPage.goto(`${started.url}#ref=${node.fnode}`);
          await otherPage.getByText("Lean editor ready", { exact: true }).waitFor();
          await manage("stop", agent); agentRunning = false;
          assert.ok(otherClosed, "stopping a branch closes its native Lean socket");
          await otherPage.close();
          assert.equal((await fetch(`${base}/p/${agent}/api/graph/check`)).status, 503);
          await cli("graph", "check");
          assert.equal(opened, 1); assert.equal(closed, 0);
          const frames = page.frames().map(frame => frame.url());
          assert.ok(frames.some(path => path.startsWith(`${url}/lean.html?`)));
          await page.getByRole("link", { name: "All projects" }).click();
          await page.getByRole("textbox", { name: "Search projects and branches" }).fill(database);
          await page.locator(`[data-project="${agent}"]`).getByText("Stopped", { exact: true }).waitFor();
          await page.getByRole("button", { name: "Stopped", exact: true }).click();
          assert.equal(await page.locator(`[data-project="${project}"]`).count(), 0);
          await page.getByRole("button", { name: "All branches", exact: true }).click();
          if (process.env.MDC_E2E_ARTIFACTS) {
            await page.screenshot({ path: resolve(process.env.MDC_E2E_ARTIFACTS, "projects-desktop.png"), fullPage: true });
            await page.getByRole("button", { name: "Toggle theme" }).click();
            await page.setViewportSize({ width: 420, height: 900 });
            assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
            await page.screenshot({ path: resolve(process.env.MDC_E2E_ARTIFACTS, "projects-mobile.png"), fullPage: true });
          }
          await page.goto(`${url}/#ref=${node.fnode}`);
          await page.getByText("Lean editor ready", { exact: true }).waitFor();
          const beforeStop = closed;
          await manage("stop");
          assert.ok(closed > beforeStop, "stopping the server closes remaining Lean sessions");
          const stopped = JSON.parse((await manage("status")).stdout);
          assert.equal(stopped.server.running, false);
          assert.equal(stopped.projects[project].running, false);
          await manage("start", project, "--port", new URL(base).port);
          await page.goto(base);
        } finally { if (agentRunning) await manage("stop", agent); }
      }, [node]);
    });
    await suite.test("graph colors follow saved Lean evidence independently of node degree and Rocq", () => {
      const nodes = [
        ["Rocq only", "rocq", "Check nat."],
        ["Empty Lean", "lean", "  \n"],
        ["Unchecked", "lean", "#check Nat\n"],
        ["Admitted", "lean", "theorem admitted : True := by sorry\n"],
        ["Proven", "lean", '-- sorry in a comment\ndef text := "sorry"\ntheorem proven : True := by trivial\n'],
      ].map(([title, srctype, content], i) => ({
        fnode: randomUUID(), title, module: `Lib.Color${i}`, depens: [], blocks: [{ srctype, content }],
      }));
      return fixture(browser, async ({ cli, page, url }) => {
        for (const title of ["Admitted", "Proven"]) {
          const check = JSON.parse((await cli("lean", "check", title)).stdout);
          assert.equal(check.certified, true);
          assert.equal(check.has_sorry, title === "Admitted");
        }
        const response = page.waitForResponse(r => r.url().endsWith("/graph/full"));
        await page.getByRole("button", { name: "Graph", exact: true }).click();
        const graph = await (await response).json();
        const expected = { "Rocq only": "no_code", "Empty Lean": "no_code", Unchecked: "unverified", Admitted: "unverified", Proven: "verified" };
        for (const node of nodes) {
          const status = graph.nodes.find(n => n.fnode === node.fnode).lean;
          assert.equal(status, expected[node.title]);
          const view = await (await fetch(`${url}/api/node/${node.fnode}/view`)).json();
          assert.equal(view.node.formalization.lean, status);
        }
        await page.getByLabel("Lean verification colors").waitFor();
        await page.waitForFunction(() => {
          const canvas = document.querySelector('.graph-container canvas');
          if (!canvas?.width) return false;
          const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
          const style = getComputedStyle(canvas);
          return ['--mdc-muted', '--mdc-warning', '--mdc-accent-down'].every(token => {
            const hex = style.getPropertyValue(token).trim().slice(1);
            const rgb = [0, 2, 4].map(i => parseInt(hex.slice(i, i + 2), 16));
            for (let i = 0; i < pixels.length; i += 4) {
              if (pixels[i] === rgb[0] && pixels[i + 1] === rgb[1] && pixels[i + 2] === rgb[2] && pixels[i + 3] === 255) return true;
            }
            return false;
          });
        });
        if (process.env.MDC_E2E_ARTIFACTS) {
          await mkdir(process.env.MDC_E2E_ARTIFACTS, { recursive: true });
          await page.screenshot({ path: resolve(process.env.MDC_E2E_ARTIFACTS, "graph-colors.png"), fullPage: true });
        }
      }, nodes);
    });
    await suite.test("native Lean bridge drains bidirectional backpressure and cancels a busy session", () => {
      const node = { fnode: randomUUID(), title: "Transport", module: "Lib.Transport", depens: [], blocks: [{ srctype: "lean", content: "#check Nat\n" }] };
      return fixture(browser, async ({ cli, url }) => {
        const current = JSON.parse((await cli("show", node.fnode)).stdout);
        const response = await fetch(`${url}/api/node/${node.fnode}/lean/session`, {
          method: "POST", headers: { "if-match": `"${current.revision}"` },
        });
        assert.equal(response.status, 200);
        const session = await response.json();
        const path = `${url}/api/lean/session/${session.id}`;
        const ws = new WebSocket(`${path.replace("http:", "ws:")}/ws`);
        const closed = new Promise(resolve => ws.addEventListener("close", resolve, { once: true }));
        const initialized = Promise.withResolvers(), drained = Promise.withResolvers();
        const replies = new Set();
        const send = message => ws.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
        ws.addEventListener("error", e => { initialized.reject(e); drained.reject(e); });
        ws.addEventListener("open", () => send({ id: 0, method: "initialize", params: { processId: null, rootUri: "file:///project", capabilities: {} } }));
        ws.addEventListener("message", ({ data }) => {
          const message = JSON.parse(data);
          if (message.method && message.id !== undefined) send({ id: message.id, result: null });
          if (message.id === 0) {
            send({ method: "initialized", params: {} }); initialized.resolve();
          } else if (!message.method && Number.isInteger(message.id)) {
            replies.add(message.id);
            if (replies.size === 2000) drained.resolve();
          }
        });
        const burst = () => {
          // Lean echoes the closed URI in its error. Both pipe directions exceed
          // the bounded queues; a single-loop bridge deadlocks on this traffic.
          const uri = `file:///${"x".repeat(65536)}`;
          for (let id = 1; id <= 2000; id++) send({ id, method: "$/lean/rpc/connect", params: { uri } });
        };
        let timer;
        try {
          await Promise.race([
            (async () => { await initialized.promise; burst(); await drained.promise; })(),
            new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`Lean transport stalled: ${replies.size}/2000 replies`)), 20000); }),
          ]);
          console.log("Native Lean backpressure replies:", replies.size);
          burst();
          assert.equal((await fetch(path, { method: "DELETE", signal: AbortSignal.timeout(5000) })).status, 204);
          await Promise.race([closed, new Promise((_, reject) => { clearTimeout(timer); timer = setTimeout(() => reject(new Error("busy Lean session did not close")), 5000); })]);
        } finally {
          clearTimeout(timer); ws.close();
          await fetch(path, { method: "DELETE" });
        }
      }, [node]);
    });
    await suite.test("native Lean editor renders goals, diagnostics and saves to the database", () => {
      const source = "theorem demo : True ∧ True := by\n  constructor\n  · trivial\n  · trivial\n";
      // Imported modules can be nested and quoted; later nodes use the flat Lib directory.
      const a = { fnode: randomUUID(), title: "Lean Example", module: "Lib.EGA.«1-1.7.1»", depens: [], blocks: [{ srctype: "lean", content: source }, { srctype: "text", content: "A shared block header." }, { srctype: "rocq", content: "Check nat." }, { srctype: "latex", content: "A formula: $x^2$." }] };
      return fixture(browser, async ({ root, page, cli, url }) => {
        let sessions = 0;
        const messages = [];
        let socket;
        page.on("websocket", ws => {
          socket = ws;
          ws.on("framesent", ({ payload }) => {
            try { messages.push(JSON.parse(String(payload))); } catch {}
          });
        });
        const nativeError = source => socket.waitForEvent("framereceived", {
          predicate: ({ payload }) => {
            const message = JSON.parse(String(payload));
            if (message.method !== "textDocument/publishDiagnostics") return false;
            const change = messages.findLast(m => m.method === "textDocument/didChange" && m.params.textDocument.uri === message.params.uri);
            // Monaco may auto-indent the final blank line after keyboard insertion.
            return change?.params.contentChanges.at(-1).text.trimEnd() === source.trimEnd() &&
              change.params.textDocument.version === message.params.version &&
              message.params.diagnostics.some(d => d.severity === 1);
          },
        });
        page.on("request", request => { if (request.method() === "POST" && request.url().endsWith("/lean/session")) sessions++; });
        let releaseSession;
        const sessionGate = new Promise(resolveGate => { releaseSession = resolveGate; });
        await page.route(/\/lean\/session$/, async route => {
          const response = await route.fetch();
          await sessionGate;
          await route.fulfill({ response });
        });
        await page.goto(`${url}/?node=${a.fnode}`);
        // Select by exact name through the same browser search used by authors.
        await page.getByRole("button", { name: /Search nodes/ }).click();
        await page.getByPlaceholder("Search by title or fnode…").fill("Lean Example");
        await page.getByRole("button", { name: /Lean Example/ }).click();
        await page.getByText("Starting Lean editor…").waitFor();
        for (const name of ["Graph", "Knowledge"]) {
          await page.getByRole("button", { name, exact: true }).click();
          await page.getByRole("button", { name, exact: true }).and(page.locator('[aria-pressed="true"]')).waitFor({ timeout: 1000 });
        }
        releaseSession();
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        await frame.locator(".monaco-editor").waitFor();
        const input = frame.getByRole("textbox", { name: /Editor content/ });
        await frame.getByText("constructor", { exact: true }).click();
        await frame.frameLocator("#infoview iframe").getByText("True", { exact: true }).first().waitFor();
        await page.locator('article[data-srctype] .block-head').nth(3).waitFor();
        const headers = await page.locator('article[data-srctype] .block-head').evaluateAll(elements => elements.map(el => {
          const s = getComputedStyle(el); return [s.minHeight, s.backgroundColor, s.fontSize, getComputedStyle(el.parentElement).borderRadius];
        }));
        assert.equal(headers.length, 4);
        for (const style of headers) assert.deepEqual(style, headers[0]);
        const leanBlock = page.locator('article[data-srctype="lean"]');
        assert.equal(await center(page).locator('.head .module-import').count(), 0);
        await leanBlock.getByText("Lean import", { exact: true }).click();
        assert.equal(await leanBlock.locator('.module-import code').textContent(), `import ${a.module}`);
        let browserChecks = 0;
        page.on("request", request => { if (request.url().endsWith("/lean/check")) browserChecks++; });
        const initializedBeforeCollapse = messages.filter(m => m.method === "initialize").length;
        await leanBlock.getByRole("button", { name: "Collapse block" }).click();
        assert.equal(await leanBlock.locator("iframe").isVisible(), false);
        await leanBlock.getByRole("button", { name: "Expand block" }).click();
        await frame.locator(".monaco-editor").waitFor();
        assert.equal(messages.filter(m => m.method === "initialize").length, initializedBeforeCollapse);
        if (process.env.MDC_E2E_ARTIFACTS) {
          await mkdir(process.env.MDC_E2E_ARTIFACTS, { recursive: true });
          await page.screenshot({ path: resolve(process.env.MDC_E2E_ARTIFACTS, "editors-wide.png"), fullPage: true });
          await page.setViewportSize({ width: 420, height: 900 });
          const boxes = await leanBlock.locator("button").evaluateAll(buttons => buttons.map(b => { const r=b.getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}; }));
          for (let i=0;i<boxes.length;i++) for(let j=i+1;j<boxes.length;j++) {
            const a=boxes[i], b=boxes[j]; assert.ok(a.x+a.w<=b.x+.5 || b.x+b.w<=a.x+.5 || a.y+a.h<=b.y+.5 || b.y+b.h<=a.y+.5, "toolbar buttons overlap");
          }
          await page.screenshot({ path: resolve(process.env.MDC_E2E_ARTIFACTS, "editors-narrow.png"), fullPage: true });
          await page.setViewportSize({ width: 1440, height: 900 });
        }
        await page.getByLabel("Lean: Verified", { exact: true }).waitFor();
        await page.screenshot({ path: resolve(root, "lean-editor.png"), fullPage: true });
        const badSource = "theorem demo : True := by\n  exact 42\n";
        const firstError = nativeError(badSource);
        await input.press("ControlOrMeta+A");
        await page.keyboard.insertText(badSource);
        await page.getByText("Unsaved", { exact: true }).waitFor();
        await firstError;
        await frame.locator(".squiggly-error").first().waitFor();
        const sessionURL = await page.locator('iframe[title="Lean source and Infoview"]').getAttribute("src");
        const switchTimes = [];
        for (let i = 0; i < 12; i++) {
          const started = performance.now();
          const name = i % 2 === 0 ? "Graph" : "Knowledge";
          await page.getByRole("button", { name, exact: true }).click();
          await page.getByRole("button", { name, exact: true }).and(page.locator('[aria-pressed="true"]')).waitFor({ timeout: 1000 });
          assert.equal(await page.locator('iframe[title="Lean source and Infoview"]').getAttribute("src"), sessionURL);
          await page.getByText("Unsaved", { exact: true }).waitFor();
          switchTimes.push(Math.round(performance.now() - started));
        }
        assert.equal(sessions, 1, "layout changes must reuse the Lean session, including a loading or dirty editor");
        console.log("Lean editor layout switch durations (ms):", switchTimes.join(", "));
        let finishSave;
        const saveGate = new Promise(resolve => { finishSave = resolve; });
        await page.route(/\/block\/lean$/, async route => { await saveGate; await route.continue(); });
        await page.getByRole("button", { name: "Graph", exact: true }).click();
        const saveButton = leanBlock.getByRole("button", { name: "Save", exact: true });
        await saveButton.click();
        const activity = page.getByRole("status").filter({ hasText: /^Saving…$/ });
        await activity.waitFor();
        assert.equal(await saveButton.textContent(), "Save");
        const buttonBox = await saveButton.boundingBox(), activityBox = await activity.boundingBox();
        assert.ok(activityBox.y >= buttonBox.y + buttonBox.height, "saving status must stay below the toolbar");
        finishSave();
        await page.unroute(/\/block\/lean$/);
        await page.getByRole("button", { name: "Knowledge", exact: true }).click();
        await page.getByText("Lean errors", { exact: false }).waitFor();
        assert.match(JSON.parse((await cli("show", a.fnode)).stdout).blocks[0].content, /exact 42/);
        await input.press("ControlOrMeta+A"); await page.keyboard.insertText("theorem demo : True ∧ True := by constructor <;> trivial\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        const savedAt = performance.now();
        await saveButton.click();
        await page.getByLabel("Lean: Verified", { exact: true }).waitFor();
        console.log("Save to automatic Verified (ms):", Math.round(performance.now()-savedAt));
        const refreshedGraph = page.waitForResponse(r => r.url().endsWith("/graph/full"));
        await page.getByRole("button", { name: "Graph", exact: true }).click();
        assert.equal((await (await refreshedGraph).json()).nodes.find(n => n.fnode === a.fnode).lean, "verified");
        await page.getByRole("button", { name: "Knowledge", exact: true }).click();
        const current = JSON.parse((await cli("show", a.fnode)).stdout);
        const cached = await (await fetch(`${url}/api/node/${a.fnode}/lean/check`, {
          method: "POST", headers: { "content-type": "application/json", "if-match": `"${current.revision}"` }, body: JSON.stringify({ build: false }),
        })).json();
        assert.equal(cached.certified, true, JSON.stringify(cached));
        assert.equal(cached.cache_hit, true, "CLI/API checks must reuse editor certification");
        assert.equal(browserChecks, 0, "saving must reuse the editor instead of starting a second checker");
        assert.equal(await leanBlock.getByRole("button", { name: /Save & (check|build)/ }).count(), 0);
        const select = async name => {
          await page.getByRole("button", { name: /Search nodes/ }).click();
          await page.getByPlaceholder("Search by title or fnode…").fill(name);
          await page.getByRole("button", { name: new RegExp(name) }).last().click();
          await title(page, name);
        };
        for (const name of ["Second Lean", "Third Lean"]) {
          const node = JSON.parse((await cli("new", "-t", name)).stdout);
          const response = await fetch(`${url}/api/node/${node.fnode}/block/lean`, {
            method: "PUT", headers: { "content-type": "application/json", "if-match": `"${node.revision}"` },
            body: JSON.stringify({ content: `theorem ${name === "Second Lean" ? "second" : "third"} : True := by trivial\n` }),
          });
          assert.equal(response.status, 200);
        }
        await select("Second Lean");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        const initialized = messages.filter(m => m.method === "initialize").length;
        const opened = messages.filter(m => m.method === "textDocument/didOpen").length;
        assert.equal(initialized, 1);
        assert.equal(opened, 2);
        const warmed = performance.now();
        await select("Lean Example");
        await frame.getByText("constructor", { exact: true }).waitFor();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        assert.equal(messages.filter(m => m.method === "initialize").length, initialized);
        assert.equal(messages.filter(m => m.method === "textDocument/didOpen").length, opened, "warm navigation must retain the native worker");
        console.log("Warm Lean node navigation (ms):", Math.round(performance.now() - warmed));
        await select("Alpha"); // A node without Lean must not tear down the runtime.
        // Let ResizeObserver see the fully hidden iframe before restoring it.
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        await select("Lean Example");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        assert.equal(sessions, 1);
        assert.equal(messages.filter(m => m.method === "textDocument/didOpen").length, opened);
        const warmSource = "theorem demo : True := by exact 42\n";
        const warmError = nativeError(warmSource);
        await input.press("ControlOrMeta+A");
        const edited = performance.now();
        await page.keyboard.insertText(warmSource);
        await warmError; // Distinguish missing native diagnostics from a hidden marker.
        await frame.locator(".squiggly-error").first().waitFor();
        console.log("Warm Lean edit diagnostics (ms):", Math.round(performance.now() - edited));
        await select("Third Lean"); // Confirmed navigation discards that unsaved edit.
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        assert.ok(messages.some(m => m.method === "textDocument/didClose"), "eviction must release a native Lean worker");
        await select("Lean Example");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await frame.getByText("constructor", { exact: true }).waitFor();
        assert.equal(await page.getByText("Unsaved", { exact: true }).count(), 0);
        for (let i = 0; i < 3; i++) {
          const old = new URL(await page.locator('iframe[title="Lean source and Infoview"]').getAttribute("src"), url).searchParams.get("session");
          await page.getByRole("button", { name: "Reload environment", exact: true }).click();
          await frame.locator(".monaco-editor").waitFor();
          assert.equal((await fetch(`${url}/api/lean/session/${old}`)).status, 404, "reload must retire the old LSP session");
        }
      }, [a]);
    });
    await suite.test("Lean startup failure is reported immediately and preserves editable source", () =>
      fixture(browser, async ({ cli, page, url }) => {
        const node = JSON.parse((await cli("show", "Alpha")).stdout);
        const response = await fetch(`${url}/api/node/${node.fnode}/block/lean`, {
          method: "PUT", headers: { "content-type": "application/json", "if-match": `"${node.revision}"` },
          body: JSON.stringify({ content: "theorem startup : True := by trivial\n" }),
        });
        assert.equal(response.status, 200);
        let fail = true, attempts = 0;
        await page.routeWebSocket(/\/api\/lean\/session\/.*\/ws$/, ws => {
          attempts++;
          const server = ws.connectToServer();
          ws.onMessage(message => server.send(message));
          server.onMessage(message => {
            if (fail && JSON.parse(String(message)).result?.capabilities) {
              ws.close({ code: 1011, reason: "fixture: native startup failed" });
              server.close();
            } else ws.send(message);
          });
        });
        await page.reload();
        await page.getByText(/Lean connection closed;/).first().waitFor({ timeout: 10000 });
        await page.waitForFunction(() => document.body.innerText.includes("Lean connection closed;") && !document.body.innerText.includes("Reconnecting Lean…"));
        assert.equal(attempts, 2, "only one automatic reconnect is allowed");
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        await frame.getByRole("textbox", { name: /Editor content/ }).press("ControlOrMeta+End");
        await page.keyboard.insertText("-- draft after failed startup\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        fail = false;
        await page.getByRole("button", { name: "Reload environment", exact: true }).click();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await frame.getByText("-- draft after failed startup", { exact: true }).waitFor();
        assert.doesNotMatch(JSON.parse((await cli("show", "Alpha")).stdout).blocks[0].content, /draft after failed startup/);
      }, [], [
        // Upstream emits shutdown rejections when initialize is interrupted.
        // Only this fault-injection fixture allows them; recovery is asserted above.
        /^Error: Client is not running and can't be stopped\. It's current state is: starting\n/,
        /^\w+: Pending response rejected since connection got disposed\n/,
      ]));
    await suite.test("Lean cancels stale selections and recovers from preparation timeouts without losing drafts", () =>
      fixture(browser, async ({ root, cli, page, url, serverOutput }) => {
        const ids = {};
        await cli("new", "-t", "Delta");
        for (const name of ["Alpha", "Gamma", "Delta"]) {
          const node = JSON.parse((await cli("show", name)).stdout);
          ids[name] = node.fnode;
          const response = await fetch(`${url}/api/node/${node.fnode}/block/lean`, {
            method: "PUT", headers: { "content-type": "application/json", "if-match": `"${node.revision}"` },
            body: JSON.stringify({ content: `theorem ${name.toLowerCase()} : True := by trivial\n` }),
          });
          assert.equal(response.status, 200);
        }
        let releaseInit, releaseSelect, initSeen, selectionHeld;
        const initializeGate = new Promise(resolve => { releaseInit = resolve; });
        const initializeSeen = new Promise(resolve => { initSeen = resolve; });
        let selectionGate = Promise.resolve();
        let blockSelect = false;
        const sent = [];
        await page.routeWebSocket(/\/api\/lean\/session\/.*\/ws$/, ws => {
          const server = ws.connectToServer();
          ws.onMessage(message => { sent.push(JSON.parse(String(message))); server.send(message); });
          server.onMessage(async message => {
            const value = JSON.parse(String(message));
            if (value.result?.capabilities) { initSeen(); await initializeGate; }
            if (value.result?.filename && blockSelect) { selectionHeld?.(); await selectionGate; }
            ws.send(message);
          });
        });
        await page.reload();
        await initializeSeen;
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        const input = frame.getByRole("textbox", { name: /Editor content/ });
        await frame.getByText("alpha", { exact: true }).waitFor({ timeout: 3000 });
        await input.press("ControlOrMeta+End");
        await page.keyboard.insertText("-- typed before Lean initialized\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        releaseInit();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await frame.getByText("-- typed before Lean initialized", { exact: true }).waitFor();
        const select = async name => {
          await page.getByRole("button", { name: /Search nodes/ }).click();
          await page.getByPlaceholder("Search by title or fnode…").fill(name);
          await page.getByRole("button", { name: new RegExp(name) }).last().click();
          await title(page, name);
        };
        blockSelect = true;
        selectionGate = new Promise(resolve => { releaseSelect = resolve; });
        await select("Gamma");
        await frame.getByText("gamma", { exact: true }).waitFor({ timeout: 1000 });
        await input.press("ControlOrMeta+End");
        await page.keyboard.insertText("-- typed while preparing node\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        blockSelect = false; releaseSelect();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await frame.getByText("-- typed while preparing node", { exact: true }).waitFor();
        await input.press("ControlOrMeta+z");
        await frame.getByText("-- typed while preparing node", { exact: true }).waitFor({ state: "hidden" });
        await input.press("ControlOrMeta+Shift+z");
        await frame.getByText("-- typed while preparing node", { exact: true }).waitFor();
        // Delay an older selection, then navigate away before its response arrives.
        blockSelect = true;
        const held = new Promise(resolve => { selectionHeld = resolve; });
        selectionGate = new Promise(resolve => { releaseSelect = resolve; });
        await select("Alpha");
        await frame.getByText("alpha", { exact: true }).waitFor({ timeout: 1000 });
        await held;
        const stale = sent.filter(m => m.method === "mdc/selectNode").at(-1).id;
        // Leave Alpha's response blocked while a cold node becomes fully ready.
        // A serial promise chain would leave Delta preparing forever here.
        blockSelect = false;
        await select("Delta");
        await frame.getByText("delta", { exact: true }).waitFor();
        await page.getByText("Lean editor ready", { exact: true }).waitFor({ timeout: 5000 });
        assert.ok(sent.some(m => m.method === "$/cancelRequest" && m.params.id === stale), "superseded selections must cancel their LSP request");
        releaseSelect();
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        await frame.getByText("delta", { exact: true }).waitFor();
        await select("Gamma");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await select("Alpha");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        blockSelect = true;
        selectionGate = new Promise(resolve => { releaseSelect = resolve; });
        await select("Gamma");
        await frame.getByText("gamma", { exact: true }).waitFor();
        await input.press("ControlOrMeta+End");
        await page.keyboard.insertText("-- draft must survive a disconnect\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        const sessionPath = () => page.locator('iframe[title="Lean source and Infoview"]').getAttribute("src");
        const oldPath = await sessionPath();
        const oldId = new URL(oldPath, url).searchParams.get("session");
        const workers = async () => (await run("ps", ["-axo", "pid=,command="])).stdout.split("\n")
          .filter(line => line.includes("--worker ") && line.includes(root)).map(line => Number(line.trim().split(/\s+/)[0]));
        const oldWorkers = await workers();
        assert.ok(oldWorkers.length > 0, "the session must have native file workers");
        // Keep the current selection unanswered. Its 30-second deadline must
        // reconnect, retire native workers, and restore the parent-owned draft.
        await page.waitForFunction(old => document.querySelector('iframe[title="Lean source and Infoview"]')?.getAttribute("src") !== old, oldPath, { timeout: 35000 });
        blockSelect = false; releaseSelect();
        assert.equal((await fetch(`${url}/api/lean/session/${oldId}`)).status, 404);
        await frame.getByText("-- draft must survive a disconnect", { exact: true }).waitFor();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await page.getByText("Unsaved", { exact: true }).waitFor();
        assert.ok(sent.filter(m => m.method === "initialize").length >= 2, "a stopped server should reconnect once");
        const remaining = await workers();
        assert.ok(oldWorkers.every(pid => !remaining.includes(pid)), "disconnect must reap the old workers, including their separate process groups");
        assert.doesNotMatch(serverOutput(), /broken pipe|recursion limit exceeded/, "disconnect must keep stdout open through native shutdown");
        assert.ok(sent.filter(m => m.method === "textDocument/didOpen").every(m => m.params.textDocument.uri.startsWith("file:///project/")), "temporary display models must not create Lean workers");
        assert.doesNotMatch(JSON.parse((await cli("show", ids.Gamma)).stdout).blocks[0].content, /draft must survive/, "reconnection must not save drafts implicitly");
      }));
    await suite.test("changed dependencies refresh the native worker without restarting its connection", () =>
      fixture(browser, async ({ cli, page, url }) => {
        const dep = JSON.parse((await cli("new", "-t", "Lean Dependency")).stdout);
        const target = JSON.parse((await cli("new", "-t", "Lean Dependent")).stdout);
        const put = async (id, content) => {
          const { node } = await (await fetch(`${url}/api/node/${id}/view`)).json();
          const response = await fetch(`${url}/api/node/${id}/block/lean`, {
            method: "PUT", headers: { "content-type": "application/json", "if-match": `"${node.revision}"` }, body: JSON.stringify({ content }),
          });
          assert.equal(response.status, 200);
        };
        await put(dep.fnode, "def anchor : Nat := 1\n");
        await put(target.fnode, `import ${dep.module}\ntheorem usesAnchor : anchor = 1 := rfl\n`);
        await cli("dep", "add", target.fnode, "--target", dep.fnode);
        const sent = [];
        page.on("websocket", ws => ws.on("framesent", ({ payload }) => {
          try { sent.push(JSON.parse(String(payload))); } catch {}
        }));
        const select = async name => {
          await page.getByRole("button", { name: /Search nodes/ }).click();
          await page.getByPlaceholder("Search by title or fnode…").fill(name);
          await page.getByRole("button", { name: new RegExp(name) }).last().click();
          await title(page, name);
        };
        await select("Lean Dependent");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await page.getByLabel("Lean: Verified", { exact: true }).waitFor();
        const leanStatus = async id => (await (await fetch(`${url}/api/node/${id}/view`)).json()).node.formalization.lean;
        assert.equal(await leanStatus(dep.fnode), "verified", "the editor's compiled imports must certify dependencies too");
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        assert.equal(await frame.locator(".squiggly-error").count(), 0);
        await put(dep.fnode, "def anchor : Nat := 2\n");
        assert.equal(await leanStatus(target.fnode), "unverified", "changing an import must invalidate the dependent certificate");
        await select("Alpha");
        await select("Lean Dependent");
        await frame.locator(".squiggly-error").first().waitFor();
        await page.getByText("Lean errors", { exact: false }).waitFor();
        assert.equal(await page.getByLabel("Lean: Verified", { exact: true }).count(), 0);
        assert.equal(await leanStatus(target.fnode), "unverified");
        assert.equal(sent.filter(m => m.method === "initialize").length, 1);
        assert.ok(sent.some(m => m.method === "textDocument/didClose"), "a changed dependency must invalidate the worker environment");
      }));
  } finally { await browser.close(); }
});
