import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { chromium } from "playwright";

const run = promisify(execFile);
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = resolve(process.env.MDC_BIN ?? resolve(webRoot, "../target/debug/mdc"));

const env = { ...process.env };
async function startServer(cwd, database) {
  const child = spawn(binary, ["serve", database, "--bind", "127.0.0.1:0"], {
    cwd, env: { ...env, MDC_CACHE_DIR: resolve(cwd, "cache") }, stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const url = await new Promise((resolveURL, reject) => {
    const timeout = setTimeout(() => reject(new Error(`server startup timeout: ${output}`)), 15000);
    child.once("error", (error) => { clearTimeout(timeout); reject(error); });
    child.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`server exited ${code}: ${output}`)); });
    child.stderr.on("data", (data) => {
      output = (output + data).slice(-16000);
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timeout); resolveURL(match[0]); }
    });
  }).catch((error) => { child.kill("SIGKILL"); throw error; });
  return { child, url, output: () => output };
}

async function fixture(browser, body) {
  const root = await mkdtemp(resolve(tmpdir(), "mdc-e2e-"));
  let server;
  let context;
  const database = `mdce2e${randomUUID().replaceAll("-", "")}`;
  const cli = (...args) => run(binary, (args[0] === "init" ? ["init", database] : args), { cwd: root, env: { ...env, ...(server ? { MDC_URL: server.url } : {}) }, timeout: 30000 });
  try {
    await cli("init");
    server = await startServer(root, database);
    for (const name of ["alpha", "beta", "gamma"]) {
      await cli("new", "-t", name[0].toUpperCase() + name.slice(1));
    }
    await cli("dep", "add", "Alpha", "--target", "Beta");
    await cli("graph", "check");
    context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await context.newPage();
    page.setDefaultTimeout(30000);
    const errors = [];
    page.on("console", message => { if (message.type() === "error") console.error("Browser console:", message.text()); });
    page.on("pageerror", (error) => errors.push(error.stack ?? error.message));
    page.on("dialog", (dialog) => void dialog.accept());
    await page.goto(server.url);
    await title(page, "Alpha");
    try { await body({ root, cli, page, url: server.url, serverOutput: server.output }); }
    catch(e) {
      await page.screenshot({ path: resolve(root, "failure.png"), fullPage: true });
      console.error("Server:", server.output());
      console.error("Browser errors:", errors);
      console.error("Page:", (await page.locator("body").innerText()).slice(-5000));
      for (const frame of page.frames().slice(1)) { console.error("Frame:", (await frame.locator("body").innerText().catch(() => "")).slice(-3000)); }
      throw e;
    }
    await cli("graph", "check");
    assert.deepEqual(errors, []);
  } finally {
    await context?.close();
    if (server && server.child.exitCode === null && server.child.signalCode === null) {
      const exited = once(server.child, "exit");
      const timeout = setTimeout(() => server.child.kill("SIGKILL"), 5000);
      server.child.kill("SIGTERM");
      await exited;
      clearTimeout(timeout);
    }
    if (env.MDC_TERMINUS_PASSWORD) {
      const response = await fetch(`${env.MDC_TERMINUS_URL ?? "http://127.0.0.1:6363"}/api/db/admin/${database}`, {
        method: "DELETE",
        headers: { Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? "admin"}:${env.MDC_TERMINUS_PASSWORD}`).toString("base64")}` },
      });
      assert.ok(response.ok || response.status === 404, `test database cleanup failed: ${response.status}`);
    }
    await rm(root, { recursive: true, force: true });
    assert.doesNotMatch(server?.output() ?? "", /broken pipe|recursion limit exceeded/, "service shutdown must finish native cleanup before closing the runtime");
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
          page.evaluate(async ({ b, c, revision }) => (await fetch(`/api/node/${c}/dep/add`, {
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
    await suite.test("native Lean editor renders goals, diagnostics and saves to the database", () =>
      fixture(browser, async ({ root, page, cli, url }) => {
        const source = "theorem demo : True ∧ True := by\n  constructor\n  · trivial\n  · trivial\n";
        // Legacy modules can be nested and quoted; later nodes use the flat Lib directory.
        const a = { fnode: randomUUID(), title: "Lean Example", module: "Lib.EGA.«1-1.7.1»", depens: [], blocks: [{ srctype: "lean", content: source }, { srctype: "text", content: "A shared block header." }, { srctype: "rocq", content: "Check nat." }, { srctype: "latex", content: "A formula: $x^2$." }] };
        const saved = await fetch(`${url}/api/import`, {
          method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ nodes: [a] }),
        });
        assert.equal(saved.status, 200);
        let sessions = 0;
        const messages = [];
        page.on("websocket", ws => ws.on("framesent", ({ payload }) => {
          try { messages.push(JSON.parse(String(payload))); } catch {}
        }));
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
        await page.screenshot({ path: resolve(root, "lean-editor.png"), fullPage: true });
        await input.press("ControlOrMeta+A");
        await page.keyboard.insertText("theorem demo : True := by\n  exact 42\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
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
        let finishCheck;
        const checkGate = new Promise(resolve => { finishCheck = resolve; });
        await page.route(/\/lean\/check$/, async route => { await checkGate; await route.continue(); });
        await page.getByRole("button", { name: "Graph", exact: true }).click();
        const checkButton = page.getByRole("button", { name: "Save & check", exact: true });
        await checkButton.click();
        const activity = page.getByRole("status").filter({ hasText: /^Checking…$/ });
        await activity.waitFor();
        assert.equal(await checkButton.textContent(), "Save & check");
        const buttonBox = await checkButton.boundingBox(), activityBox = await activity.boundingBox();
        assert.ok(activityBox.y >= buttonBox.y + buttonBox.height, "checking status must be below the toolbar, without overlapping button labels");
        finishCheck();
        await page.unroute(/\/lean\/check$/);
        await page.getByRole("button", { name: "Knowledge", exact: true }).click();
        await page.getByText("Lean errors", { exact: false }).waitFor();
        assert.match(JSON.parse((await cli("show", a.fnode)).stdout).blocks[0].content, /exact 42/);
        await input.press("ControlOrMeta+A"); await page.keyboard.insertText("theorem demo : True ∧ True := by constructor <;> trivial\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        const buildResponse=page.waitForResponse(r => r.url().endsWith("/lean/check") && r.request().method()==="POST");
        await page.getByRole("button", { name: "Save & build", exact: true }).click();
        const built=await (await buildResponse).json();
        assert.equal(built.certified,true,JSON.stringify(built));
        await page.getByText("olean ready", { exact: false }).waitFor();
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
        await select("Lean Example");
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        assert.equal(sessions, 1);
        assert.equal(messages.filter(m => m.method === "textDocument/didOpen").length, opened);
        await input.press("ControlOrMeta+A");
        const edited = performance.now();
        await page.keyboard.insertText("theorem demo : True := by exact 42\n");
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
      }));
    await suite.test("Lean stays editable during delayed startup and selection, and reconnects without losing drafts", () =>
      fixture(browser, async ({ root, cli, page, url, serverOutput }) => {
        const ids = {};
        for (const name of ["Alpha", "Gamma"]) {
          const node = JSON.parse((await cli("show", name)).stdout);
          ids[name] = node.fnode;
          const response = await fetch(`${url}/api/node/${node.fnode}/block/lean`, {
            method: "PUT", headers: { "content-type": "application/json", "if-match": `"${node.revision}"` },
            body: JSON.stringify({ content: `theorem ${name.toLowerCase()} : True := by trivial\n` }),
          });
          assert.equal(response.status, 200);
        }
        let releaseInit, releaseSelect, initSeen;
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
            if (value.result?.fnode && blockSelect) await selectionGate;
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
        selectionGate = new Promise(resolve => { releaseSelect = resolve; });
        await select("Alpha");
        await frame.getByText("alpha", { exact: true }).waitFor({ timeout: 1000 });
        await select("Gamma");
        blockSelect = false; releaseSelect();
        await frame.getByText("gamma", { exact: true }).waitFor();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
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
        assert.equal((await fetch(`${url}/api/lean/session/${oldId}`, { method: "DELETE" })).status, 204);
        await page.waitForFunction(old => document.querySelector('iframe[title="Lean source and Infoview"]')?.getAttribute("src") !== old, oldPath);
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
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        assert.equal(await frame.locator(".squiggly-error").count(), 0);
        await put(dep.fnode, "def anchor : Nat := 2\n");
        await select("Alpha");
        await select("Lean Dependent");
        await frame.locator(".squiggly-error").first().waitFor();
        assert.equal(sent.filter(m => m.method === "initialize").length, 1);
        assert.ok(sent.some(m => m.method === "textDocument/didClose"), "a changed dependency must invalidate the worker environment");
      }));
  } finally { await browser.close(); }
});
