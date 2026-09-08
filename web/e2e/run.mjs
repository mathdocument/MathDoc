import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, rm } from "node:fs/promises";
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
try {
  for (const line of (await readFile(resolve(webRoot, "../.env"), "utf8")).split("\n")) {
    if (line.startsWith("MDC_") && line.includes("=")) {
      const at = line.indexOf("="); env[line.slice(0, at)] ??= line.slice(at + 1);
    }
  }
} catch (e) { if (e.code !== "ENOENT") throw e; }
async function startServer(cwd, database) {
  const child = spawn(binary, ["--database", database, "serve", "--bind", "127.0.0.1:0"], {
    cwd, env, stdio: ["ignore", "pipe", "pipe"],
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
  const cli = (...args) => run(binary, ["--database", database, ...args], { cwd: root, env: { ...env, ...(server ? { MDC_URL: server.url } : {}) }, timeout: 30000 });
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
    try { await body({ root, cli, page, url: server.url }); }
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
        const a = JSON.parse((await cli("new", "-t", "Lean Example")).stdout);
        const source = "theorem demo : True ∧ True := by\n  constructor\n  · trivial\n  · trivial\n";
        const saved = await fetch(`${url}/api/node/${a.fnode}/block/lean`, {
          method: "PUT", headers: { "content-type": "application/json", "if-match": `"${a.revision}"` }, body: JSON.stringify({ content: source }),
        });
        assert.equal(saved.status, 200);
        let sessions = 0;
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
        await page.getByRole("button", { name: "Save & check", exact: true }).click();
        await page.getByText("Lean errors", { exact: false }).waitFor();
        assert.match(JSON.parse((await cli("show", a.fnode)).stdout).blocks[0].content, /exact 42/);
        await input.press("ControlOrMeta+A"); await page.keyboard.insertText("theorem demo : True ∧ True := by constructor <;> trivial\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        const buildResponse=page.waitForResponse(r => r.url().endsWith("/lean/check") && r.request().method()==="POST");
        await page.getByRole("button", { name: "Save & build", exact: true }).click();
        const built=await (await buildResponse).json();
        assert.equal(built.certified,true,JSON.stringify(built));
        await page.getByText("olean ready", { exact: false }).waitFor();
        for (let i = 0; i < 3; i++) {
          const old = new URL(await page.locator('iframe[title="Lean source and Infoview"]').getAttribute("src"), url).searchParams.get("session");
          await page.getByRole("button", { name: "Reload environment", exact: true }).click();
          await frame.locator(".monaco-editor").waitFor();
          assert.equal((await fetch(`${url}/api/lean/session/${old}`)).status, 404, "reload must retire the old LSP session");
        }
      }));
  } finally { await browser.close(); }
});
