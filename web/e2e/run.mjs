import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import { chromium } from "playwright";

const run = promisify(execFile);
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = resolve(process.env.MDC_BIN ?? resolve(webRoot, "../target/debug/mdc"));

async function startServer(cwd) {
  const child = spawn(binary, ["serve", "alpha", "--bind", "127.0.0.1:0"], {
    cwd, stdio: ["ignore", "pipe", "pipe"],
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
  return { child, url };
}

async function fixture(browser, body) {
  const root = await mkdtemp(resolve(tmpdir(), "mdc-e2e-"));
  let server;
  let context;
  const cli = (...args) => run(binary, args, { cwd: root, timeout: 15000 });
  try {
    await cli("init");
    for (const name of ["alpha", "beta", "gamma"]) {
      await cli("new", "-t", name[0].toUpperCase() + name.slice(1), "-f", name);
    }
    await cli("dep", "add", "alpha", "--target", "beta");
    await cli("graph", "check");
    server = await startServer(root);
    context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await context.newPage();
    page.setDefaultTimeout(10000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("dialog", (dialog) => void dialog.accept());
    await page.goto(server.url);
    await title(page, "Alpha");
    await body({ root, cli, page, url: server.url });
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

await test("browser with the real MathDoc backend", { timeout: 120000 }, async (suite) => {
  const browser = await chromium.launch({ headless: true, channel: "chromium" });
  try {
    await suite.test("external edits reject a stale save and preserve the browser draft", () =>
      fixture(browser, async ({ root, page }) => {
        await rename(page, "Browser Alpha");
        const path = resolve(root, "alpha.mdoc");
        await writeFile(path, (await readFile(path, "utf8")).replace("@title: Alpha", "@title: External Alpha"));
        const response = page.waitForResponse((response) => response.url().endsWith("/title") && response.request().method() === "PUT");
        await page.getByRole("button", { name: "Save title", exact: true }).click();
        assert.equal((await response).status(), 412);
        await page.getByText("resource changed; refresh and retry", { exact: true }).waitFor();
        assert.equal(await page.getByRole("textbox", { name: "Node title" }).inputValue(), "Browser Alpha");
        assert.match(await readFile(path, "utf8"), /@title: External Alpha/);
      }));

    await suite.test("navigation waits for an in-flight save before switching nodes", () =>
      fixture(browser, async ({ root, page }) => {
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
        assert.match(await readFile(resolve(root, "alpha.mdoc"), "utf8"), /@title: Saved Alpha/);
        assert.match(await readFile(resolve(root, "beta.mdoc"), "utf8"), /@title: Beta/);
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
        const b = await resolveNode("beta");
        const c = await resolveNode("gamma");
        const view = await (await fetch(`${url}/api/node/${c}/view`)).json();
        const results = await Promise.all([
          cli("dep", "add", "beta", "--target", "gamma").then(() => true, () => false),
          page.evaluate(async ({ b, c, revision }) => (await fetch(`/api/node/${c}/dep/add`, {
            method: "POST", headers: { "content-type": "application/json", "if-match": `"${revision}"` },
            body: JSON.stringify({ dep_fnode: b }),
          })).ok, { b, c, revision: view.node.revision }),
        ]);
        assert.equal(results.filter(Boolean).length, 1);
        const refreshed = page.waitForResponse((response) => response.url().endsWith("/workspace/refresh"));
        await page.getByRole("button", { name: "Refresh external file changes" }).click();
        const report = await (await refreshed).json();
        assert.deepEqual(report.cycles, []);
        assert.equal(report.edges, 2);
      }));
  } finally { await browser.close(); }
});
