import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { chromium, firefox, webkit } from "playwright";
import { createServer } from "node:net";

// Browser tests against the MathDoc backend (coordinator/src/main.ts) and its worker.
// Test data is built through the HTTP API; `cli(...)` is a thin shim over the same API.
// Requires MDC_DATABASE_URL and a TerminusDB (MDC_TERMINUS_URL, MDC_TERMINUS_PASSWORD).
const run = promisify(execFile);
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(webRoot, "..");
const env = { ...process.env };
const documentEndKey = process.platform === "darwin" ? "Meta+ArrowDown" : "Control+End";
function launchBrowser() {
  if (env.MDC_E2E_BROWSER === 'firefox') return firefox.launch();
  if (env.MDC_E2E_BROWSER === 'webkit') return webkit.launch();
  return chromium.launch({headless: true, channel: 'chromium'});
}

// One actor for the whole suite; the browser receives the same token.
const E2E_ACTOR = "e2e";
const E2E_TOKEN = `e2e-${randomUUID()}`;
const TOKEN_KEY = "mdc-access-token";
let backend;
async function startBackend() {
  if (backend) return backend;
  const reservation = createServer();
  await new Promise(resolve => reservation.listen(0, "127.0.0.1", resolve));
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const tsx = resolve(repoRoot, "node_modules/.bin/tsx");
  const processEnv = {
    ...env,
    MDC_PORT: String(port),
    MDC_HOST: "127.0.0.1",
    MDC_ACTORS: JSON.stringify({ [E2E_ACTOR]: { token: E2E_TOKEN, admin: true } }),
    MDC_WEB_DIR: resolve(webRoot, "dist"),
    LEANGROUND_SERVER_URL: env.LEANGROUND_SERVER_URL ?? "http://127.0.0.1:9",
    LEANGROUND_FACT_TOKEN: env.LEANGROUND_FACT_TOKEN ?? "unused",
    LEANGROUND_ACTOR: env.LEANGROUND_ACTOR ?? "coordinator",
    // Never install the LaTeX runtime implicitly during tests.
    MDC_LATEX_PYTHON: env.MDC_LATEX_PYTHON ?? "/nonexistent/mdc-latex-runtime",
  };
  await run(tsx, ["coordinator/src/main.ts", "migrate"], { cwd: repoRoot, env: processEnv, timeout: 60000 });
  const child = spawn(tsx, ["coordinator/src/main.ts", "serve"], { cwd: repoRoot, env: processEnv, stdio: ["ignore", "pipe", "pipe"] });
  let log = "";
  const collect = chunk => { log = (log + chunk).slice(-64000); };
  child.stdout.on("data", collect);
  child.stderr.on("data", collect);
  const deadline = Date.now() + 30000;
  while (!log.includes("MathDoc http://")) {
    if (child.exitCode !== null || Date.now() > deadline) throw new Error(`backend did not start:\n${log}`);
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  // The collaboration worker runs next to the API, as in a deployment (plan §8).
  const worker = spawn(tsx, ["coordinator/src/main.ts", "worker"], { cwd: repoRoot, env: processEnv, stdio: ["ignore", "pipe", "pipe"] });
  worker.stdout.on("data", collect);
  worker.stderr.on("data", collect);
  // Shared by every test in this file: stop them when the file finishes, and never let
  // them keep the finished test process alive.
  for (const p of [child, worker]) {
    p.unref();
    p.stdout.unref?.();
    p.stderr.unref?.();
    process.once("exit", () => p.kill());
  }
  backend = { child, base: `http://127.0.0.1:${port}`, output: () => log.slice(-16000) };
  return backend;
}

// Requests from this process to the backend carry the suite's token unless they set their own.
const nativeFetch = globalThis.fetch;
globalThis.fetch = (input, init = {}) => {
  const target = String(input instanceof Request ? input.url : input);
  if (backend && target.startsWith(backend.base)) {
    const headers = new Headers(init.headers);
    if (!headers.has("authorization")) headers.set("authorization", `Bearer ${E2E_TOKEN}`);
    return nativeFetch(input, { ...init, headers });
  }
  return nativeFetch(input, init);
};

async function api(url, method = "GET", body, revision) {
  const headers = { "content-type": "application/json" };
  if (revision) headers["if-match"] = `"${revision}"`;
  const response = await fetch(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await response.text();
  if (!response.ok) throw new Error(`${method} ${url}: ${response.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

/** The mdc CLI commands used by these tests, over HTTP. Output mirrors `{stdout}`. */
function cliFor(base, database) {
  const project = `${base}/p/${database}/main/api`;
  const view = async ref => (await api(`${project}/node/${encodeURIComponent(ref)}/view`)).node;
  const out = value => ({ stdout: JSON.stringify(value) });
  return async (...args) => {
    const [command, ...rest] = args;
    if (command === "init") return out(await api(`${base}/api/projects`, "POST", { action: "init", name: database }));
    if (command === "export") return out(await api(`${project}/export`));
    if (command === "import") return out(await api(`${project}/import`, "POST", JSON.parse(await readFile(rest[0], "utf8"))));
    if (command === "graph" && rest[0] === "check") return out(await api(`${project}/graph/check`));
    if (command === "show") return out(await view(rest[0]));
    if (command === "rename") { const n = await view(rest[0]); return out(await api(`${project}/node/${n.fnode}/title`, "PUT", { title: rest[1] }, n.revision)); }
    if (command === "del") { const n = await view(rest[0]); return out(await api(`${project}/node/${n.fnode}`, "DELETE", undefined, n.revision)); }
    if (command === "new" && rest[0] === "-t") return out(await api(`${project}/node/new`, "POST", { title: rest[1] }));
    if (command === "dep" && rest[0] === "add" && rest[2] === "--target") {
      const n = await view(rest[1]);
      return out(await api(`${project}/node/${n.fnode}/dep/add`, "POST", { dep_fnode: rest[3] }, n.revision));
    }
    if (command === "branch" && rest[0] === "new") return out(await api(`${project}/branches`, "POST", { name: rest[1] }));
    if (command === "project" && rest[0] === "latex" && rest[1] === "show") return out(await api(`${project}/project/latex`));
    if (command === "project" && rest[0] === "latex" && rest[1] === "set" && rest[2] === "--preamble" && rest[4] === "--bib") {
      // Like `mdc project latex set`: file names become the project file names.
      const current = await api(`${project}/project/latex`);
      return out(await api(`${project}/project/latex`, "PUT", {
        preamble_name: basename(rest[3]), preamble: await readFile(rest[3], "utf8"),
        bibliography_name: basename(rest[5]), bibliography: await readFile(rest[5], "utf8"),
      }, current.revision));
    }
    throw new Error(`cli shim does not implement: ${args.join(" ")}`);
  };
}

async function tsFixture(browser, body, extraNodes = [], expectedPageErrors = []) {
  const root = await mkdtemp(resolve(tmpdir(), "mdc-e2e-"));
  let context;
  const server = await startBackend();
  const database = `mdce2e${randomUUID().replaceAll("-", "")}`;
  const cli = cliFor(server.base, database);
  const url = `${server.base}/p/${database}/main`;
  try {
    await cli("init");
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
    // Sandboxed frames (LaTeX previews) have no storage; only the app document needs the token.
    await context.addInitScript(([key, token]) => { try { localStorage.setItem(key, token); } catch { /* sandboxed frame */ } }, [TOKEN_KEY, E2E_TOKEN]);
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
    page.on("pageerror", (error) => errors.push((error.stack || error.message).replace(/^Unhandled Promise Rejection: /, "")));
    page.on("dialog", (dialog) => void dialog.accept());
    try {
      await page.goto(`${url}/#ref=${bundle.nodes[0].fnode}`);
      await title(page, "Alpha");
      await body({ root, cli, page, url, serverOutput: server.output });
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
    if (env.MDC_TERMINUS_PASSWORD) {
      const response = await nativeFetch(`${env.MDC_TERMINUS_URL ?? "http://127.0.0.1:6363"}/api/db/admin/${database}`, {
        method: "DELETE",
        headers: { Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? "admin"}:${env.MDC_TERMINUS_PASSWORD}`).toString("base64")}` },
      });
      assert.ok(response.ok || response.status === 404, `test database cleanup failed: ${response.status}`);
    }
    await rm(root, { recursive: true, force: true });
  }
}

const fixture = tsFixture;

const center = (page) => page.getByRole("region", { name: "current node" });
async function title(page, name) {
  await center(page).getByRole("button", { name, exact: true }).waitFor();
}
async function leanReply(socket, method) {
  const sent = await socket.waitForEvent("framesent", {predicate: ({payload}) => JSON.parse(String(payload)).method === method});
  const {id} = JSON.parse(String(sent.payload));
  const received = await socket.waitForEvent("framereceived", {predicate: ({payload}) => JSON.parse(String(payload)).id === id});
  const reply = JSON.parse(String(received.payload));
  assert.equal(reply.error, undefined, JSON.stringify(reply.error));
  return reply.result;
}
async function rename(page, value) {
  await center(page).getByTitle("Click to rename").click();
  await page.getByRole("textbox", { name: "Node title" }).fill(value);
}
const beta = (page) => page.getByRole("complementary", { name: "Dependencies" })
  .getByRole("button", { name: /^Beta \(/ });

await test('project directory creates, forks and deletes branches and projects', {timeout: 90000}, async () => {
  const browser = await launchBrowser();
  try {
    await fixture(browser, async ({page, url}) => {
      const base = new URL(url).origin, main = new URL(url).pathname.slice(3);
      const database = main.split('/')[0], fork = `${database}/draft`;
      const initialized = `mdce2einit${randomUUID().replaceAll('-', '')}`;
      const row = name => page.locator(`[data-project="${name}"]`);
      await page.goto(base);
      await page.getByRole('textbox', {name: 'Search projects and branches'}).fill(database);
      // Branches load on demand: every readable branch is listed with the caller's role.
      await row(main).getByText('admin', {exact: true}).waitFor();
      assert.equal(await page.getByText('MAIN', {exact: true}).count(), 0);
      assert.match(await row(main).locator('.counts').innerText(), /3.*nodes/);
      assert.match(await row(main).locator('.counts').innerText(), /1.*edge/);
      assert.equal(await row(main).getByRole('button', {name: `Delete ${main}`, exact: true}).count(), 0);
      assert.equal(await row(main).locator('.main-branch').evaluate(el => getComputedStyle(el).borderTopStyle), 'solid');
      const branch = async (source, name) => {
        await row(source).getByRole('button', {name: `New branch from ${source}`, exact: true}).click();
        await page.getByLabel('Branch name', {exact: true}).fill('invalid/name');
        assert.equal(await page.getByLabel('Branch name', {exact: true}).evaluate(el => el.checkValidity()), false);
        await page.getByLabel('Branch name', {exact: true}).fill(name);
        await page.getByRole('dialog').getByRole('button', {name: 'New branch', exact: true}).click();
        await page.getByRole('dialog').waitFor({state: 'hidden'});
        await page.getByRole('textbox', {name: 'Search projects and branches'}).fill(database);
      };
      await branch(main, 'draft');
      await row(fork).waitFor();
      await branch(fork, 'copy');
      await row(`${database}/copy`).waitFor();
      // Opening a fork loads it; the directory then shows its counts.
      assert.equal((await fetch(`${base}/p/${fork}/api/graph/check`)).ok, true);
      await page.getByRole('button', {name: 'Refresh projects'}).click();
      await page.waitForFunction(name => /3.*nodes/.test(document.querySelector(`[data-project="${name}"] .counts`)?.textContent ?? ''), fork);
      for (const theme of ['dark', 'light']) {
        if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) await page.getByRole('button', {name: 'Toggle theme'}).click();
        for (const width of [1440, 750, 420]) {
          await page.setViewportSize({width, height: 900});
          assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth || document.querySelector('.directory').scrollWidth > innerWidth), false);
          for (const selector of ['.counts', '.branch-actions', '.branch-actions button', '.open, .open-space']) {
            const bounds = await Promise.all([main, fork].map(name => row(name).locator(selector).first().boundingBox()));
            assert.equal(bounds[0].x, bounds[1].x, `${selector} aligns at ${width}px`);
            assert.equal(bounds[0].width + (selector === '.branch-actions' ? 30 : 0), bounds[1].width);
          }
          await page.screenshot({path: resolve(tmpdir(), `mdc-projects-${theme}-${width}-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`), fullPage: true});
        }
      }
      await page.setViewportSize({width: 1440, height: 900});
      await row(fork).getByRole('button', {name: `Delete ${fork}`, exact: true}).click();
      await row(fork).waitFor({state: 'hidden'});
      assert.equal((await fetch(`${url}/api/graph/check`)).ok, true, 'other branches remain usable');
      try {
        await page.getByRole('button', {name: 'Init project', exact: true}).click();
        await page.getByLabel('Project name', {exact: true}).fill(initialized);
        await page.getByRole('dialog').getByRole('button', {name: 'Init', exact: true}).click();
        await page.getByRole('dialog').waitFor({state: 'hidden'});
        await row(`${initialized}/main`).waitFor();
        assert.equal(await row(`${initialized}/main`).getByRole('button', {name: `Delete ${initialized}/main`, exact: true}).count(), 0);
        // Identity replaces the legacy same-origin-only guard: a token is required, a foreign
        // Origin is refused, and removal requires the database_delete role.
        const body = JSON.stringify({action: 'remove', database: initialized});
        assert.equal((await nativeFetch(`${base}/api/projects`, {method: 'POST', headers: {'content-type': 'application/json'}, body})).status, 401);
        assert.equal((await fetch(`${base}/api/projects`, {method: 'POST', headers: {'content-type': 'application/json', origin: 'https://attacker.invalid'}, body})).status, 403);
        assert.ok((await fetch(`${base}/api/projects`, {method: 'POST', headers: {'content-type': 'application/json', origin: base},
          body: JSON.stringify({action: 'new_branch', project: `${initialized}/main`, name: 'hidden'})})).ok);
        await page.getByRole('textbox', {name: 'Search projects and branches'}).fill(initialized);
        const removeProject = page.getByRole('button', {name: `Delete project ${initialized}`, exact: true});
        // Cancel must preserve the project; accepting removes every branch.
        page.removeAllListeners('dialog');
        page.once('dialog', dialog => void dialog.dismiss());
        await removeProject.click();
        assert.ok((await fetch(`${base}/p/${initialized}/main/api/graph/check`)).ok);
        const confirmations = [];
        page.on('dialog', dialog => { confirmations.push(dialog.message()); void dialog.accept(); });
        const rejectRemoval = route => route.request().postDataJSON()?.action === 'remove'
          ? route.fulfill({status: 500, json: {error: 'test removal failed'}}) : route.continue();
        await page.route('**/api/projects', rejectRemoval);
        await removeProject.click();
        await page.getByRole('alert').filter({hasText: 'test removal failed'}).waitFor();
        await page.unroute('**/api/projects', rejectRemoval);
        await removeProject.click();
        await page.getByRole('region', {name: `Project ${initialized}`, exact: true}).waitFor({state: 'hidden'});
        assert.ok(confirmations.every(text => text.includes('All its branches') && text.includes('history')));
        assert.ok((await fetch(`${url}/api/graph/check`)).ok, 'removing a project preserves other projects');
        const inventory = await (await fetch(`${base}/api/projects`)).json();
        assert.equal(Object.keys(inventory.projects).some(name => name.startsWith(`${initialized}/`)), false);
        assert.equal(await page.getByRole('alert').filter({hasText: 'test removal failed'}).count(), 0);
        const db = await nativeFetch(`${env.MDC_TERMINUS_URL ?? 'http://127.0.0.1:6363'}/api/db/admin/${initialized}`, {
          headers: {Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? 'admin'}:${env.MDC_TERMINUS_PASSWORD}`).toString('base64')}`},
        });
        assert.equal(db.status, 404);
      } finally {
        const removed = await nativeFetch(`${env.MDC_TERMINUS_URL ?? 'http://127.0.0.1:6363'}/api/db/admin/${initialized}`, {
          method: 'DELETE', headers: {Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? 'admin'}:${env.MDC_TERMINUS_PASSWORD}`).toString('base64')}`},
        });
        assert.ok(removed.ok || removed.status === 404);
      }
      await page.route('**/api/projects', route => route.fulfill({json: {server: {running: true}, projects: {}}}));
      await page.goto(base);
      await page.getByRole('heading', {name: 'No projects yet'}).waitFor();
      assert.equal(await page.getByRole('button', {name: 'Init project', exact: true}).isEnabled(), true);
    });
  } finally { await browser.close(); }
});

await test("large relation lists filter and retain natural card heights", { timeout: 90000 }, async () => {
  const browser = await launchBrowser();
  const node = (title, fnode = randomUUID(), depens = []) => ({ fnode, title, module: `Lib.N_${fnode.replaceAll("-", "")}`, depens, blocks: [] });
  const hub = node("Shared foundation");
  const leaves = Array.from({length: 8500}, (_, i) => node(
    `Entry ${String(i).padStart(5, "0")}${i % 3 === 0 ? " with a deliberately long mathematical title that wraps across two lines" : ""}`,
    `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`, [hub.fnode]));
  const root = node("Large collection", undefined, leaves.map(n => n.fnode));
  try {
    await fixture(browser, async ({page, url}) => {
      const started = performance.now();
      await page.goto(`${url}/?relations#ref=${root.fnode}`);
      await title(page, root.title);
      const column = page.getByRole("complementary", {name: "Dependencies", exact: true});
      const cards = column.locator("button.card");
      const card = (id) => column.locator(`button.card[data-fnode="${id}"]`);
      await card(leaves[1].fnode).waitFor();
      assert.ok(await cards.count() < 80, "large columns only mount a viewport of cards");
      const measure = async locator => locator.evaluate(el => el.getBoundingClientRect().height);
      const shortHeight = await measure(card(leaves[1].fnode));
      const longHeight = await measure(card(leaves[0].fnode));
      assert.ok(longHeight > shortHeight + 10, `${longHeight} vs ${shortHeight}: short titles do not reserve an empty second line`);
      const filter = column.getByRole("searchbox", {name: "Filter dependencies"});
      await filter.fill("ENTRY 00001");
      await page.waitForFunction(() => document.querySelector('[aria-label="Dependencies"] .count')?.textContent === "1/8500");
      assert.equal(await cards.count(), 1);
      assert.ok(Math.abs(await measure(card(leaves[1].fnode)) - shortHeight) < 0.5, "small and virtual lists share the same card height");
      await filter.fill(leaves.at(-1).fnode);
      await card(leaves.at(-1).fnode).waitFor();
      assert.equal(await cards.count(), 1);
      await filter.fill("no such node");
      await column.getByText("No matching nodes", {exact: true}).waitFor();
      await filter.fill("");
      await card(leaves[0].fnode).focus();
      await page.keyboard.press("End");
      await page.waitForFunction(id => document.activeElement?.getAttribute("data-fnode") === id, leaves.at(-1).fnode);
      const contiguous = async () => {
        await page.waitForFunction(() => {
          const rows = [...document.querySelectorAll('[aria-label="Dependencies"] li[data-fnode]')].map(el => el.getBoundingClientRect());
          return rows.length > 1 && rows.slice(1).every((row, i) => Math.abs(row.top - rows[i].bottom) < 0.5);
        });
      };
      await contiguous();
      await page.keyboard.press("Home");
      await page.waitForFunction(id => document.activeElement?.getAttribute("data-fnode") === id, leaves[0].fnode);
      await page.setViewportSize({width: 1100, height: 800});
      await contiguous();
      await page.setViewportSize({width: 1440, height: 900});
      await contiguous();
      await page.getByRole("button", {name: "Graph", exact: true}).click();
      await page.getByRole("button", {name: "Knowledge", exact: true}).click();
      await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
      await card(leaves[0].fnode).waitFor();
      assert.ok(await cards.count() < 80);
      await contiguous();
      await page.screenshot({path: resolve(tmpdir(), "mdc-large-node-lists.png")});
      console.log("Large-list rendering, filtering, scrolling and view-switch checks (ms):", Math.round(performance.now() - started));
      await card(leaves[1].fnode).click();
      await title(page, leaves[1].title);
      assert.equal(await filter.inputValue(), "", "filters reset on node navigation");
      await column.getByRole("button", {name: /^Shared foundation /}).click();
      await title(page, hub.title);
      const refs = page.getByRole("complementary", {name: "Referrers", exact: true});
      assert.ok(await refs.locator("button.card").count() < 80);
      await refs.getByRole("searchbox", {name: "Filter referrers"}).fill("ENTRY 08499");
      await refs.locator(`button.card[data-fnode="${leaves.at(-1).fnode}"]`).waitFor();
      assert.equal(await refs.locator("button.card").count(), 1);
    }, [hub, root, ...leaves]);
  } finally { await browser.close(); }
});

await test('node operation dialogs share layout, focus and Escape handling', {timeout: 60000}, async () => {
  const browser = await launchBrowser();
  try {
    await fixture(browser, async ({page}) => {
      const toolbar = page.locator('.bar-end');
      const open = async (button, label) => {
        await toolbar.getByRole('button', {name: button, exact: true}).press('Enter');
        const dialog = page.getByRole('dialog', {name: label, exact: true});
        await dialog.evaluate(el => Promise.all(el.getAnimations().map(animation => animation.finished)));
        return dialog;
      };
      for (const theme of ['light', 'dark']) {
        if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) await page.getByRole('button', {name: `Switch to ${theme} mode`}).click();
        for (const width of [1440, 420]) {
          await page.setViewportSize({width, height: 900});
          const layouts = [];
          for (const [button, label, query] of [
            ['Create node', 'new node', ''],
            ['Add dependency', 'add dependency', 'Gamma'],
            ['Remove dependency', 'remove dependencies', 'Beta'],
          ]) {
            const dialog = await open(button, label);
            assert.equal(await dialog.locator('input').evaluate(el => el === document.activeElement), true, `${label} focuses its input`);
            layouts.push(await dialog.evaluate(el => {
              const box = el.getBoundingClientRect(), head = el.querySelector('.dialog-head').getBoundingClientRect();
              const close = el.querySelector('.close-btn').getBoundingClientRect();
              return {width: box.width, top: box.top, headerHeight: head.height, closeRight: close.right, radius: getComputedStyle(el).borderRadius};
            }));
            assert.ok(layouts.at(-1).width <= width, 'dialog fits the viewport');
            assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth), true, 'dialog contents fit narrow screens');
            await dialog.locator('input').fill(query);
            await page.screenshot({path: resolve(tmpdir(), `mdc-${label.replaceAll(' ', '-')}-${theme}-${width}-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
            await dialog.locator('input').press('Escape');
            await dialog.waitFor({state: 'hidden'});
            assert.equal(await toolbar.getByRole('button', {name: button, exact: true}).evaluate(el => el === document.activeElement), true, 'closing returns focus to the trigger');
          }
          assert.deepEqual(layouts[0], layouts[1]);
          assert.deepEqual(layouts[1], layouts[2]);
        }
      }
      await page.setViewportSize({width: 1440, height: 900});
      // Escape follows the same unsaved-draft protection as the close button.
      page.removeAllListeners('dialog');
      const create = await open('Create node', 'new node');
      await create.locator('input').fill('Unsaved title');
      page.once('dialog', dialog => void dialog.dismiss());
      await create.locator('input').press('Escape');
      assert.equal(await create.isVisible(), true);
      assert.equal(await create.locator('input').inputValue(), 'Unsaved title');
      page.once('dialog', dialog => void dialog.accept());
      await create.locator('input').press('Escape');
      await create.waitFor({state: 'hidden'});
      const add = await open('Add dependency', 'add dependency');
      await add.locator('input').fill('New dependency draft');
      await add.getByRole('button', {name: 'Create new: New dependency draft'}).click();
      page.once('dialog', dialog => void dialog.dismiss());
      await add.locator('input').press('Escape');
      assert.equal(await add.getByRole('button', {name: 'create & add'}).isVisible(), true);
      page.once('dialog', dialog => void dialog.accept());
      await add.locator('input').press('Escape');
      await add.waitFor({state: 'hidden'});
      await beta(page).click();
      await title(page, 'Beta');
      assert.equal(await toolbar.getByRole('button', {name: 'Remove dependency', exact: true}).isDisabled(), true, 'nodes without dependencies cannot open the removal dialog');
    });
  } finally { await browser.close(); }
});

await test("browser with the real MathDoc backend", { timeout: 240000 }, async (suite) => {
  const browser = await launchBrowser();
  try {
    await suite.test("node deletion detaches referrers and dependency dialogs share their layout", () =>
      fixture(browser, async ({ cli, page, url }) => {
        const alpha = JSON.parse((await cli("show", "Alpha")).stdout);
        const toolbar = page.locator(".bar-end");
        const positions = await Promise.all(["Create node", "Add dependency", "Remove dependency", "Delete node"].map(async name =>
          (await toolbar.getByRole("button", { name, exact: true }).boundingBox()).x));
        assert.deepEqual(positions, [...positions].sort((a, b) => a - b));
        assert.equal(await toolbar.getByRole("button", { name: "Create node", exact: true }).innerText(), "");
        await toolbar.getByRole("button", { name: "Add dependency", exact: true }).click();
        const add = page.getByRole("dialog", { name: "add dependency", exact: true });
        const addHeader = await add.locator(".dialog-head").evaluate(el => ({width: el.offsetWidth, height: el.offsetHeight}));
        await add.getByRole("searchbox").fill("Gamma");
        await add.getByRole("button", { name: /Gamma/ }).click();
        await toolbar.getByRole("button", { name: "Remove dependency", exact: true }).click();
        const remove = page.getByRole("dialog", { name: "remove dependencies", exact: true });
        const removeHeader = await remove.locator(".dialog-head").evaluate(el => ({width: el.offsetWidth, height: el.offsetHeight}));
        assert.equal(addHeader.width, removeHeader.width);
        assert.equal(addHeader.height, removeHeader.height);
        await remove.getByRole("button", { name: /Gamma/ }).click();
        await remove.getByRole("button", { name: "Remove selected", exact: true }).click();
        await cli("dep", "add", "Gamma", "--target", "Beta");
        await beta(page).click();
        await title(page, "Beta");
        await cli("rename", "Beta", "Renamed Beta");
        await toolbar.getByRole("button", { name: "Delete node", exact: true }).click();
        await page.getByText("node changed; reload and retry", { exact: true }).waitFor();
        assert.equal(JSON.parse((await cli("show", "Alpha")).stdout).depens.length, 1);
        await toolbar.getByRole("button", { name: "Refresh database view", exact: true }).click();
        await title(page, "Renamed Beta");
        await toolbar.getByRole("button", { name: "Delete node", exact: true }).click();
        await page.waitForFunction(() => document.querySelector(".graph-stats")?.textContent.includes("2 nodes · 0 edges"));
        assert.deepEqual(JSON.parse((await cli("show", "Alpha")).stdout).depens, []);
        assert.deepEqual(JSON.parse((await cli("show", "Gamma")).stdout).depens, []);
        const deleted = JSON.parse((await cli("del", "Gamma")).stdout);
        assert.equal(deleted.deleted, true);
        await page.goto(`${url}/?after-deletion#ref=${alpha.fnode}`);
        await title(page, "Alpha");
        await toolbar.getByRole("button", { name: "Delete node", exact: true }).click();
        await page.getByText("This project has no nodes. Create one with the + button.", { exact: true }).waitFor();
        assert.equal(await toolbar.getByRole("button", { name: "Delete node", exact: true }).isDisabled(), true);
        await toolbar.getByRole("button", { name: "Create node", exact: true }).click();
        const create = page.getByRole("dialog", { name: "new node", exact: true });
        await create.getByPlaceholder("New Lemma").fill("Recreated");
        await create.getByRole("button", { name: "Create node", exact: true }).click();
        await title(page, "Recreated");
      }));

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

    // The CLI side is the HTTP shim: two clients race opposite edges on the same revision.
    await suite.test("CLI and browser cannot concurrently introduce opposite edges", () =>
      fixture(browser, async ({ cli, page, url }) => {
        const resolveNode = async (ref) => (await (await fetch(`${url}/api/resolve?ref=${ref}`)).json()).fnode;
        const b = await resolveNode("Beta");
        const c = await resolveNode("Gamma");
        const view = await (await fetch(`${url}/api/node/${c}/view`)).json();
        const results = await Promise.all([
          cli("dep", "add", "Beta", "--target", "Gamma").then(() => true, () => false),
          page.evaluate(async ({ b, c, revision }) => (await fetch(`${location.pathname.replace(/\/$/, "")}/api/node/${c}/dep/add`, {
            method: "POST", headers: { "content-type": "application/json", "if-match": `"${revision}"`,
              authorization: `Bearer ${localStorage.getItem("mdc-access-token")}` },
            body: JSON.stringify({ dep_fnode: b }),
          })).ok, { b, c, revision: view.node.revision }),
        ]);
        assert.equal(results.filter(Boolean).length, 1);
        const refreshed = page.waitForResponse((response) => response.url().endsWith("/graph/check"));
        await page.getByRole("button", { name: "Refresh database view" }).click();
        const report = await (await refreshed).json();
        assert.deepEqual(report, { nodes: 3, edges: 2 });
      }));
  } finally { await browser.close(); }
});

await test('Monaco source blocks retain highlighting, edits and undo across layout changes', {timeout: 60000}, async () => {
  const browser = await launchBrowser();
  const node = {fnode: randomUUID(), title: 'Shared editors', module: 'Lib.Shared', depens: [], blocks: [
    {srctype: 'text', content: '# Heading\nA **bold** statement.\n'},
    {srctype: 'latex', content: '% comment\n\\section{Heading}\nA statement.\n'},
    {srctype: 'rocq', content: '(* comment *)\nTheorem demo : True. Proof. exact I. Qed.\n'},
  ]};
  try {
    await fixture(browser, async ({page, url, cli}) => {
      let sessions = 0;
      page.on('request', r => { if (r.method() === 'POST' && r.url().endsWith('/lean/session')) sessions++; });
      await page.goto(`${url}/?shared#ref=${node.fnode}`);
      for (const {srctype, content} of node.blocks) {
        const block = page.locator(`[data-srctype="${srctype}"]`);
        await block.locator('.editor-scroll:not(.pending)').waitFor();
        const colors = await block.locator('.view-line span').evaluateAll(spans => [...new Set(spans.map(el => getComputedStyle(el).color))]);
        assert.ok(colors.length >= 2, `${srctype} has native syntax colors`);
        const input = block.getByRole('textbox', {name: new RegExp(`^${srctype} source`)});
        await input.press(documentEndKey);
        await page.keyboard.insertText('draft');
        await block.getByText('Unsaved', {exact: true}).waitFor();
        await page.getByRole('button', {name: 'Graph', exact: true}).click();
        await page.waitForFunction(() => document.querySelector('button[title="Graph view"]')?.getAttribute('aria-pressed') === 'true' && !document.querySelector('.app[inert]'));
        await input.press('ControlOrMeta+z');
        await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
        assert.equal(JSON.parse((await cli('show', node.fnode)).stdout).blocks.find(b => b.srctype === srctype).content, content);
        await page.getByRole('button', {name: 'Knowledge', exact: true}).click();
        await page.waitForFunction(() => document.querySelector('button[title="Knowledge view"]')?.getAttribute('aria-pressed') === 'true' && !document.querySelector('.app[inert]'));
      }
      await page.getByRole('button', {name: 'Switch to dark mode'}).click();
      for (const {srctype} of node.blocks) await page.locator(`[data-srctype="${srctype}"] .monaco-editor[role=code].vs-dark`).waitFor();
      assert.equal(sessions, 0, 'ordinary editing never starts a Lean server');
    }, [node]);
  } finally { await browser.close(); }
});

await test('LaTeX macros, scoped completion, citations and draft previews', {timeout: 90000}, async () => {
  const browser = await launchBrowser();
  try {
    await fixture(browser, async ({root, cli, page, url}) => {
      const ids = {};
      const put = async (name, content) => {
        const node = JSON.parse((await cli('show', name)).stdout);
        ids[name] = node.fnode;
        const response = await fetch(`${url}/api/node/${node.fnode}/block/latex`, {
          method: 'PUT', headers: {'content-type': 'application/json', 'if-match': `"${node.revision}"`}, body: JSON.stringify({content}),
        });
        assert.equal(response.status, 200);
      };
      await page.getByRole('button', {name: 'Project settings', exact: true}).click();
      await page.getByRole('tab', {name: 'LaTeX', exact: true}).click();
      const preamble = resolve(root, 'macros.tex'), bib = resolve(root, 'references.bib');
      await writeFile(preamble, String.raw`\usepackage{amsthm}\newcommand{\cA}{\mathcal{A}}\newenvironment{items}{\begin{itemize}}{\end{itemize}}\newtheorem{thm}{Theorem}`);
      await writeFile(bib, String.raw`@article{paper,title={A paper on {M}\"{o}bius},author={Author, A.},journal={Journal},year={2020}}` +
        Array.from({length: 14000}, (_, i) => `@article{zextra${i},title={Another publication ${i}},author={Writer, B.},year={2021}}`).join('\n'));
      await page.getByRole('button', {name: 'Save LaTeX project', exact: true}).waitFor();
      await page.getByLabel('Class or preamble').setInputFiles(preamble);
      await page.getByLabel('Bibliography', {exact: true}).setInputFiles(bib);
      await page.getByRole('button', {name: 'Save LaTeX project', exact: true}).click();
      await page.getByRole('dialog').waitFor({state: 'hidden'});
      const settings = JSON.parse((await cli('project', 'latex', 'show')).stdout).project;
      assert.match(settings.preamble, /newcommand/);
      assert.match(settings.bibliography, /A paper/);
      const source = String.raw`\section{Introduction}\label{intro}By \nameref{thm:b}, see \cite{paper}. $\cA$ The value $r=0$ is zero. Blackboard bold: $\mathbb{ABCDEFGHIJKLMNOPQRSTUVWXYZ}$.\[\left(\frac{x_1^2}{1+x}\right)=\begin{pmatrix}a&b\\c&d\end{pmatrix}\]\begin{items}\item One\end{items}`;
      await put('Alpha', source);
      await put('Beta', String.raw`\begin{thm}[Named result]\label{thm:b}$\cA$ exists.\end{thm}`);
      await put('Gamma', String.raw`\section{Private result}\label{private}`);
      const externalName = number => `${ids.Beta.slice(0, 8)}::Theorem ${number}`;
      await page.reload();
      const block = page.locator('article[data-srctype="latex"]');
      await block.getByText('1 imported dependencies', {exact: false}).waitFor();
      await block.locator('.latex-imports summary').click();
      const requests = [];
      page.on('request', request => requests.push(request.url()));
      const failedMathAssets = [];
      page.on('response', response => { if (response.url().includes('/mathjax/') && response.status() >= 400) failedMathAssets.push(response.url()); });
      let releaseFont;
      const fontGate = new Promise(resolve => { releaseFont = resolve; });
      await page.route('**/mathjax/**/chtml.js', async route => { await fontGate; await route.continue(); });
      const fontRequest = page.waitForRequest('**/mathjax/**/chtml.js');
      await block.getByRole('button', {name: 'Render LaTeX preview'}).click();
      await fontRequest;
      await block.getByRole('button', {name: 'Return to LaTeX editor'}).click();
      releaseFont();
      await page.waitForFunction(() => typeof window.MathJax?.typesetPromise === 'function');
      assert.equal(await block.locator('mjx-container').count(), 0, 'a closed preview stays closed when fonts finish loading');
      await block.getByRole('button', {name: 'Render LaTeX preview'}).click();
      assert.equal(await block.locator('.latex-imports summary').innerText(), '1 imported dependencies');
      await block.locator('.latex-imports li').getByText('Beta', {exact: true}).waitFor();
      await block.getByRole('link', {name: externalName(1), exact: true}).waitFor();
      await block.locator('mjx-container[jax="CHTML"]').first().waitFor();
      assert.equal(await block.locator('mjx-merror, .latex-error').count(), 0);
      assert.equal(await block.locator('mjx-mfrac').count(), 1);
      assert.equal(await block.locator('mjx-mtable').count(), 1);
      assert.ok(await block.locator('mjx-assistive-mml math').count() > 0, 'retain accessible MathML');
      const inline = await block.locator('.latex-math[data-tex="r=0"] mjx-container').evaluate(element => ({
        math: getComputedStyle(element).fontSize, text: getComputedStyle(element.closest('p')).fontSize,
        weight: getComputedStyle(element).fontWeight,
      }));
      assert.equal(inline.math, inline.text, 'inline math must not be enlarged relative to surrounding text');
      assert.equal(inline.weight, '400');
      await page.evaluate(() => document.fonts.ready);
      const blackboard = block.locator('mjx-mi[data-latex="ABCDEFGHIJKLMNOPQRSTUVWXYZ"]');
      assert.equal(await blackboard.locator('mjx-c').count(), 26);
      assert.ok(await blackboard.locator('mjx-c').evaluateAll(chars => chars.every(char =>
        getComputedStyle(char).fontFamily.includes('MJX-TEX-N'))), 'use the AMS alphabet from the official TeX font');
      const blackboardRWidth = await blackboard.locator('.mjx-c211D').evaluate(char =>
        char.getBoundingClientRect().width / parseFloat(getComputedStyle(char).fontSize));
      assert.ok(Math.abs(blackboardRWidth - 0.722) < 0.003, 'AMS msbm10 R has width 0.722em, unlike Latin Modern Math at 0.639em');
      assert.ok(requests.some(request => request.includes('/mathjax-tex-font/chtml/woff2/')), 'use the official Computer Modern/AMS math fonts');
      assert.ok(requests.every(request => new URL(request).origin === new URL(url).origin), 'all renderer and font assets are self-hosted');
      assert.deepEqual(failedMathAssets, [], 'all requested MathJax assets are bundled');
      assert.match(await block.locator('.latex-preview').innerText(), /A paper/);
      const bibliographyTitle = block.locator('.latex-bibliography em');
      assert.equal(await bibliographyTitle.innerText(), 'A paper on Möbius');
      const fonts = await bibliographyTitle.evaluate(async element => {
        const loaded = await Promise.all(['400', 'italic 400', '700', 'italic 700'].map(style =>
          document.fonts.load(`${style} 16px "Latin Modern Roman"`, 'Möbius é ü')));
        const css = getComputedStyle(element);
        return {family: css.fontFamily, style: css.fontStyle,
          loaded: loaded.map(faces => faces.length === 1 && faces[0].status === 'loaded')};
      });
      assert.equal(fonts.family.split(',')[0].replace(/["']/g, '').trim(), 'Latin Modern Roman');
      assert.equal(fonts.style, 'italic');
      assert.deepEqual(fonts.loaded, [true, true, true, true], 'all four complete TeX font faces are served');
      if (process.env.MDC_E2E_BROWSER !== 'webkit') {
        const cdp = await page.context().newCDPSession(page);
        await cdp.send('DOM.enable');
        await cdp.send('CSS.enable');
        const {root: dom} = await cdp.send('DOM.getDocument');
        const {nodeId} = await cdp.send('DOM.querySelector', {nodeId: dom.nodeId, selector: '.latex-bibliography em'});
        const {fonts: used} = await cdp.send('CSS.getPlatformFontsForNode', {nodeId});
        assert.equal(used.length, 1, 'accented letters must not fall back to another font');
        assert.equal(used[0].postScriptName, 'LMRoman10-Italic');
        assert.equal(used[0].isCustomFont, true);
        await cdp.detach();
      }
      await page.screenshot({path: resolve(tmpdir(), `mdc-latex-font-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
      const back = page.getByRole('button', {name: 'Back', exact: true});
      const forward = page.getByRole('button', {name: 'Forward', exact: true});
      assert.equal(await back.isDisabled(), true);
      assert.equal(await forward.isDisabled(), true);
      // A slow renderer must leave the current readable node in place, not
      // replace it with a new heading and "Preparing preview...".
      let releasePreview, previewRequested;
      const previewGate = new Promise(resolve => { releasePreview = resolve; });
      const previewStarted = new Promise(resolve => { previewRequested = resolve; });
      const previewPath = `**/node/${ids.Beta}/latex/preview`;
      const previousPreviews = requests.filter(path => path.endsWith(`/node/${ids.Beta}/latex/preview`)).length;
      await page.route(previewPath, async route => { previewRequested(); await previewGate; await route.continue(); });
      await page.evaluate(() => {
        window.previewLoadingFrames = 0;
        window.watchPreview = true;
        const sample = () => {
          if (!window.watchPreview) return;
          if (document.querySelector('.preview-loading')?.getBoundingClientRect().height) window.previewLoadingFrames++;
          requestAnimationFrame(sample);
        };
        requestAnimationFrame(sample);
      });
      try {
        await block.getByRole('link', {name: externalName(1), exact: true}).click();
        await previewStarted;
        await title(page, 'Alpha');
        assert.equal(await block.getByRole('heading', {name: 'Introduction', exact: true}).isVisible(), true);
      } finally { releasePreview(); await page.unroute(previewPath); }
      await title(page, 'Beta');
      await block.locator('.latex-preview .latex-statement').waitFor();
      await block.locator('mjx-container[jax="CHTML"]').waitFor();
      assert.equal(await block.locator('.monaco-editor[role=code]').count(), 0, 'reading a new node does not create a hidden source editor');
      assert.equal(await page.evaluate(() => { window.watchPreview = false; return window.previewLoadingFrames; }), 0);
      assert.equal(requests.filter(path => path.endsWith(`/node/${ids.Beta}/latex/preview`)).length - previousPreviews, 1, 'reuse the prepared response after mounting');
      assert.equal(await page.evaluate(() => [...window.MathJax.startup.document.math].length), 1, 'discard the previous node math when navigating');
      assert.equal(await block.locator('[id="latex-thm%3Ab"]').count(), 1);
      assert.equal(await block.locator('.latex-statement-title').innerText(), 'Theorem 1 (Named result)');
      const headingGap = await block.locator('.latex-statement').evaluate(element => {
        const title = element.querySelector('.latex-statement-title').getClientRects()[0];
        const paragraph = element.querySelector('p').getClientRects()[0];
        return Math.abs(title.y - paragraph.y);
      });
      assert.ok(headingGap < 4, 'theorem heading and first paragraph must share a line');
      assert.equal(await forward.isDisabled(), true);
      await back.click();
      await title(page, 'Alpha');
      assert.equal(await back.isDisabled(), true);
      assert.equal(await forward.isDisabled(), false);
      await forward.click();
      await title(page, 'Beta');
      await block.locator('.latex-preview .latex-statement').waitFor();
      await back.click();
      await title(page, 'Alpha');
      await block.locator('.latex-imports summary').getByText('1 imported dependencies', {exact: true}).waitFor();
      await block.getByRole('heading', {name: 'Introduction', exact: true}).waitFor();
      // Reading mode belongs to this page, not the node or other browser tabs.
      const other = await page.context().newPage();
      try {
        await other.goto(`${url}/#ref=${ids.Alpha}`);
        await other.getByRole('textbox', {name: /^latex source/}).waitFor();
        assert.equal(await other.locator('.latex-preview').count(), 0);
      } finally { await other.close(); }
      await block.getByRole('button', {name: 'Return to LaTeX editor'}).click();
      await block.getByRole('textbox', {name: /^latex source/}).waitFor();
      await beta(page).click();
      await title(page, 'Beta');
      await block.getByRole('textbox', {name: /^latex source/}).waitFor();
      assert.equal(await block.locator('.latex-preview').count(), 0);
      await page.goBack();
      await title(page, 'Alpha');
      await block.locator('.latex-imports summary').getByText('1 imported dependencies', {exact: true}).waitFor();
      const input = block.getByRole('textbox', {name: /^latex source/});
      const fill = async value => { await input.press('ControlOrMeta+A'); await page.keyboard.insertText(value); };
      // First paint without hovering: Safari 27 used to show a blank command
      // list until individual rows were invalidated by the pointer.
      await page.mouse.move(0, 0);
      await fill('\\');
      const commands = page.getByRole('listbox', {name: 'LaTeX suggestions'});
      await commands.waitFor();
      assert.ok(await commands.getByRole('option').count() > 1);
      await page.screenshot({path: resolve(tmpdir(), `mdc-command-completion-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
      await input.pressSequentially('mathbb');
      await commands.getByRole('option', {selected: true}).filter({hasText: /^mathbb$/}).waitFor();
      await input.press('Enter');
      assert.equal(await commands.count(), 0, 'accepting a command hides the popup');
      assert.match(await block.locator('.view-lines').innerText(), /\\mathbb/);
      await fill('Use \\nameref{thm:b');
      await input.press('Control+Space');
      await page.getByRole('option', {selected: true}).filter({hasText: 'Named result'}).waitFor();
      await input.press('Enter');
      const qualifiedReference = `\\nameref{${ids.Beta}::thm:b`;
      await page.waitForFunction(text => document.querySelector('[data-srctype="latex"] .view-lines').innerText.includes(text), qualifiedReference);
      assert.ok((await block.locator('.view-lines').innerText()).includes(qualifiedReference));
      assert.equal(await page.getByRole('option').filter({hasText: 'Private result'}).count(), 0);
      for (const theme of ['dark', 'light']) {
        await page.getByRole('button', {name: `Switch to ${theme} mode`}).click();
        await fill('% filler\n'.repeat(7) + '\\cite{');
        await input.press('Control+Space');
        const option = page.getByRole('option').filter({hasText: 'A paper'});
        await option.waitFor();
        const popup = page.locator('.mdc-editor-widgets .latex-completions:visible');
        assert.equal(await popup.count(), 1, 'completion escapes the source block clipping context');
        assert.equal(await popup.getByRole('option').count(), 50, 'all 50 candidates are mounted, including offscreen rows');
        assert.equal(await popup.evaluate(el => getComputedStyle(el).overflowY), 'auto');
        const geometry = await option.evaluate(el => {
          const add = document.querySelector('.add-btn').getBoundingClientRect();
          const widget = el.closest('.latex-completions');
          const row = widget.getBoundingClientRect();
          const left = Math.max(row.left, add.left), right = Math.min(row.right, add.right);
          const top = Math.max(row.top, add.top), bottom = Math.min(row.bottom, add.bottom);
          return {
            overlap: right > left && bottom > top,
            onTop: widget.contains(document.elementFromPoint((left + right) / 2, (top + bottom) / 2)),
            radius: parseFloat(getComputedStyle(widget).borderRadius),
          };
        });
        assert.equal(geometry.overlap, true, 'exercise completion over the add source block button');
        assert.equal(geometry.onTop, true, 'the completion receives clicks above the add button');
        assert.ok(geometry.radius >= 8, 'use the app popup shape');
        assert.equal(await page.locator('.suggest-details:visible').count(), 0, 'no duplicate details panel');
        const popupBox = await popup.boundingBox();
        assert.equal(await popup.evaluate(el => { el.scrollTop = 100; return el.scrollTop; }), 100, 'use native scrolling');
        await popup.evaluate(el => { el.scrollTop = 0; });
        await page.mouse.move(popupBox.x + 100, popupBox.y + 70);
        await popup.evaluate(el => {
          window.completionFrames = []; window.recordCompletion = true;
          window.completionRows = [...el.children];
          const sample = () => {
            const box = el.getBoundingClientRect();
            window.completionFrames.push({x: box.x, y: box.y, height: box.height, shown: getComputedStyle(el).visibility,
              list: el.scrollTop, retained: window.completionRows.every((row, i) => el.children[i] === row), pane: document.querySelector('.blocks').scrollTop,
              hit: el.contains(document.elementFromPoint(box.x + 100, box.y + 70))});
            if (window.recordCompletion) requestAnimationFrame(sample);
          }; requestAnimationFrame(sample);
        });
        for (const direction of [1, -1]) for (let step = 0; step < 10; step++) {
          await page.mouse.wheel(0, direction * 30);
          await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
        }
        const frames = await page.evaluate(() => {window.recordCompletion = false; return window.completionFrames;});
        assert.ok(frames.some(frame => frame.list > 100), 'scroll through native citation candidates');
        assert.ok(frames.every(frame => frame.x === popupBox.x && frame.y === popupBox.y && frame.height === popupBox.height && frame.shown === 'visible' && frame.hit), 'completion geometry and hit testing stay stable through every scroll frame');
        assert.ok(frames.every(frame => frame.retained), 'never remove or recycle candidate rows while scrolling');
        assert.equal(new Set(frames.map(frame => frame.pane)).size, 1, 'candidate scrolling never moves the page underneath');
        await page.screenshot({path: resolve(tmpdir(), `mdc-completion-${theme}-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
        await option.click();
        await page.waitForFunction(() => /cite\{paper/.test(document.querySelector('[data-srctype="latex"] .view-lines').innerText));
        assert.match(await block.locator('.view-lines').innerText(), /cite\{paper/);
      }
      // Search the full catalog while typing and expand again on backspace.
      await fill('\\cite{');
      await input.press('Control+Space');
      await page.getByRole('option').filter({hasText: 'A paper'}).waitFor();
      await input.pressSequentially('zextra13999');
      await page.getByRole('option').filter({hasText: 'Another publication 13999'}).waitFor();
      assert.equal(await page.getByRole('option').first().locator('span').first().innerText(), 'zextra13999');
      await input.press('Backspace');
      await page.getByRole('option').filter({hasText: 'Another publication 13998'}).waitFor();
      await input.press('9');
      await page.getByRole('option').filter({hasText: 'Another publication 13999'}).click();
      assert.match(await block.locator('.view-lines').innerText(), /cite\{zextra13999/);
      await fill('\\cite{z13999');
      await input.press('Control+Space');
      await page.getByRole('option').filter({hasText: 'Another publication 13999'}).waitFor();
      assert.equal(await page.getByRole('option').first().locator('span').first().innerText(), 'zextra13999', 'native fuzzy ranking finds noncontiguous keys across the full catalog');
      await fill('\\cite{A paper');
      await input.press('Control+Space');
      await page.getByRole('option').filter({hasText: 'A paper'}).waitFor();
      await input.press('Enter');
      await page.waitForFunction(() => /cite\{paper/.test(document.querySelector('[data-srctype="latex"] .view-lines').innerText));
      assert.match(await block.locator('.view-lines').innerText(), /cite\{paper/);
      await input.press('ControlOrMeta+z');
      await page.waitForFunction(() => /cite\{A.paper/.test(document.querySelector('[data-srctype="latex"] .view-lines').innerText));
      assert.match(await block.locator('.view-lines').innerText(), /cite\{A.paper/);
      await input.press('Escape');
      assert.equal(await page.locator('.latex-completions:visible').count(), 0);
      await fill('\\cite{');
      await input.press('Control+Space');
      await page.getByRole('option').filter({hasText: 'A paper'}).waitFor();
      await input.press('ArrowDown');
      const nextKey = await page.getByRole('option', {selected: true}).locator('span').first().innerText();
      await input.press('Tab');
      await page.waitForFunction(key => document.querySelector('[data-srctype="latex"] .view-lines').innerText.includes(`cite{${key}`), nextKey);
      assert.ok((await block.locator('.view-lines').innerText()).includes(`cite{${nextKey}`));
      const draft = String.raw`\section{Draft title}\label{new}By \nameref{thm:b}, see \cite{paper}. $\cA$`;
      await fill(draft);
      const discard = page.listeners('dialog');
      page.removeAllListeners('dialog');
      try {
        const prompted = page.waitForEvent('dialog');
        page.once('dialog', dialog => void dialog.dismiss());
        await forward.click();
        await prompted;
        await page.waitForFunction(() => window.history.state?.index === 0 && document.querySelector('.app')?.getAttribute('aria-busy') === 'false');
        await title(page, 'Alpha');
        assert.match(await block.locator('.view-lines').innerText(), /Draft.title/);
        assert.equal(await back.isDisabled(), true);
        assert.equal(await forward.isDisabled(), false);
      } finally { discard.forEach(listener => page.on('dialog', listener)); }
      await block.getByRole('button', {name: 'Render LaTeX preview'}).click();
      await block.getByRole('heading', {name: 'Draft title', exact: true}).waitFor();
      assert.equal(JSON.parse((await cli('show', 'Alpha')).stdout).blocks[0].content, source);
      // Dependency numbering refreshes without reloading or saving the draft.
      await put('Beta', String.raw`\begin{thm}Earlier result.\end{thm}\begin{thm}[Updated result]\label{thm:b}Updated.\end{thm}`);
      await block.getByRole('link', {name: externalName(2), exact: true}).waitFor({timeout: 10000});
      await block.getByRole('button', {name: 'Save', exact: true}).click();
      await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
      assert.equal(JSON.parse((await cli('show', 'Alpha')).stdout).blocks[0].content, draft);
      const saved = JSON.parse((await cli('show', 'Alpha')).stdout).blocks[0].content;
      assert.doesNotMatch(saved, /externaldocument/);
      await page.goto("about:blank");
      await page.goto(`${url}/#ref=${ids.Beta}&label=thm%3Ab`);
      await block.getByText('Theorem 2 (Updated result)', {exact: true}).waitFor();
      if (process.env.MDC_E2E_ARTIFACTS) await page.screenshot({path: resolve(process.env.MDC_E2E_ARTIFACTS, `latex-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
    });
  } finally { await browser.close(); }
});

await test('LaTeX tables, colors and TikZ diagrams render shared macros locally', {timeout: 90000}, async () => {
  const browser = await launchBrowser();
  const node = {fnode: randomUUID(), title: 'Rich LaTeX', module: 'Lib.RichLatex', depens: [], blocks: [{srctype: 'latex', content: String.raw`
    {\color{brand}Colored text} and $\textcolor{brand}{x+y}$.
    \begin{tabular}{|l|c|}\hline Left & Right\\\hline \multicolumn{2}{c}{Together}\\\hline\end{tabular}
    \begin{tikzcd}\cA \arrow[r,"f",color=brand] \arrow[d,"g"'] & B \arrow[d,"h"] \\ C \arrow[r,"k"'] & D\end{tikzcd}
  `}]};
  try {
    await fixture(browser, async ({page, url, cli, root}) => {
      const preamble = resolve(root, 'diagrams.tex');
      await writeFile(preamble, String.raw`\renewcommand\headrulewidth{0pt}\newcommand{\alphabet}[1]{\mathcal{#1}}\newcommand{\cA}{\alphabet{A}}\definecolor{brand}{HTML}{336699}`);
      const bibliography = resolve(root, 'diagrams.bib');
      await writeFile(bibliography, '');
      await cli('project', 'latex', 'set', '--preamble', preamble, '--bib', bibliography);
      const requests = [];
      page.on('request', request => requests.push(request.url()));
      const failedMathAssets = [];
      page.on('response', response => { if (response.url().includes('/mathjax/') && response.status() >= 400) failedMathAssets.push(response.url()); });
      await page.goto(`${url}/?diagrams#ref=${node.fnode}`);
      await page.getByRole('button', {name: 'Render LaTeX preview'}).click();
      await page.locator('.latex-diagram svg').waitFor({timeout: 45000});
      await page.locator('.latex-math mjx-container').waitFor();
      assert.equal(await page.locator('.latex-error, mjx-merror').count(), 0);
      assert.equal(await page.locator('.latex-math mjx-mstyle').first().evaluate(el => getComputedStyle(el).color), 'rgb(51, 102, 153)');
      assert.equal(await page.locator('.latex-table td[colspan="2"]').innerText(), 'Together');
      assert.equal(await page.locator('.latex-preview').getByText('Colored text', {exact: true}).evaluate(el => getComputedStyle(el).color), 'rgb(51, 102, 153)');
      assert.ok(await page.locator('.latex-diagram svg path').count() > 0);
      assert.match(await page.locator('.latex-diagram svg').innerHTML(), /336699|51.*,.*102.*,.*153/i);
      assert.ok(requests.filter(url => /tex_files/.test(url)).length > 0, 'loads the bundled TikZ runtime');
      assert.ok(requests.every(request => new URL(request).origin === new URL(url).origin), 'no CDN or external rendering service');
      assert.deepEqual(failedMathAssets, [], 'all requested MathJax assets are bundled');
      await page.screenshot({path: resolve(tmpdir(), `mdc-rich-latex-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
      let previewVersion = 0;
      const preview = async source => {
        await page.getByRole('button', {name: 'Return to LaTeX editor'}).click();
        const input = page.getByRole('textbox', {name: /^latex source/});
        const marker = `Preview version ${++previewVersion}`;
        await input.press('ControlOrMeta+A'); await page.keyboard.insertText(source + '\n\\par ' + marker);
        await page.getByRole('button', {name: 'Render LaTeX preview'}).click();
        await page.locator('.latex-preview').getByText(marker, {exact: true}).waitFor();
      };
      await preview(String.raw`\[
        \begin{tikzcd}
        A&B\\
        C&D
        \end{tikzcd}
      \]`);
      await page.locator('.latex-diagram svg').waitFor();
      assert.equal(await page.locator('mjx-merror, .latex-diagram.latex-error').count(), 0);
      assert.equal(await page.locator('.latex-diagram svg text').count(), 4);
      await preview(String.raw`\[
        \begin{tikzcd}
        A\ar[r] & B\ar[d] \\
        C & D\ar[l]
        \end{tikzcd}
      \]`);
      await page.locator('.latex-diagram svg').waitFor();
      const visibleStrokes = () => page.locator('.latex-diagram svg path').evaluateAll(paths =>
        paths.filter(path => getComputedStyle(path).stroke !== 'none').length);
      assert.equal(await visibleStrokes(), 6, 'three visible arrow shafts and three arrowheads');
      await page.screenshot({path: resolve(tmpdir(), `mdc-tikzcd-arrows-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
      await preview(String.raw`\begin{equation}X = \begin{tikzcd}A \arrow[r] & B\end{tikzcd}\end{equation}`);
      await page.locator('.latex-diagram svg').waitFor();
      assert.ok(await page.locator('.latex-diagram svg text').count() >= 4, 'surrounding X = is preserved with the diagram');
      assert.equal(await visibleStrokes(), 2, 'surrounding math must not hide the arrow');
      await preview(String.raw`\begin{tikzcd}\notAMdcMacro\end{tikzcd}`);
      await page.locator('.latex-diagram.latex-error').waitFor();
      assert.match(await page.locator('.latex-diagram').innerText(), /Undefined control sequence/);
      await preview(String.raw`\begin{tikzcd}E \arrow[r] & F\end{tikzcd}`);
      await page.locator('.latex-diagram svg').waitFor();
      await preview(String.raw`\begin{tikzcd}A \pgfextra{\special{dvisvgm:raw <script>window.diagramInjected=1</script><image href="https://invalid.example/pixel"/><circle style="fill:url(https://invalid.example/color)"/>}} \arrow[r] & B\end{tikzcd}`);
      await page.locator('.latex-diagram svg').waitFor();
      assert.equal(await page.evaluate(() => window.diagramInjected), undefined);
      assert.equal(await page.locator('.latex-diagram script, .latex-diagram image').count(), 0);
      assert.doesNotMatch(await page.locator('.latex-diagram').innerHTML(), /invalid\.example/);
      await preview(String.raw`\begin{tikzcd}\input{https://invalid.example/secret} \end{tikzcd}`);
      await page.locator('.latex-diagram.latex-error').waitFor({timeout: 40000});
      assert.ok(requests.every(request => new URL(request).origin === new URL(url).origin), 'TeX cannot fetch arbitrary input URLs');
      await preview(String.raw`\newcommand{\tic}[2]{\begin{tabular}{@{}#1@{}}#2\end{tabular}}
        \tic{c|c}{A&B}
        \par\tic{c|c}{\toprule A&B\\\midrule C&D\\\bottomrule}
        \par\tic{c|c}{\toprule[2pt] E&F\\\bottomrule[2pt]}`);
      assert.equal(await page.locator('.latex-error').count(), 0);
      const tables = page.locator('.latex-table table');
      assert.equal(await tables.count(), 3);
      for (const theme of ['dark', 'light']) {
        await page.getByRole('button', {name: `Switch to ${theme} mode`}).click();
        const cells = await tables.first().locator('td').evaluateAll(elements => elements.map(el => {
          const style = getComputedStyle(el);
          return {align: style.textAlign, border: style.borderRightStyle, width: parseFloat(style.borderRightWidth),
            borderColor: style.borderRightColor, color: style.color, left: style.paddingLeft, right: style.paddingRight};
        }));
        assert.equal(cells[0].align, 'center');
        assert.equal(cells[0].border, 'solid');
        assert.ok(cells[0].width > 0, 'the c|c separator survives leading @{}');
        assert.equal(cells[0].borderColor, cells[0].color, 'table rules remain visible in either theme');
        assert.equal(cells[0].left, '0px');
        assert.equal(cells[1].right, '0px');
        const rules = await tables.nth(1).locator('td').evaluateAll(elements => elements.map(el => ({
          top: el.style.borderTopWidth, bottom: el.style.borderBottomWidth,
        })));
        assert.equal(rules[0].top, '0.08em');
        assert.equal(rules[2].top, '0.05em');
        assert.equal(rules[2].bottom, '0.08em');
        const explicit = await tables.nth(2).locator('td').first().evaluate(el => getComputedStyle(el).borderTopWidth);
        assert.ok(parseFloat(explicit) >= 2, 'explicit booktabs rule widths are honored');
        await page.screenshot({path: resolve(tmpdir(), `mdc-table-rules-${theme}-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
      }
    }, [node]);
  } finally { await browser.close(); }
});

await test('Lean blocks are source editors certified by LeanGround, without local Lean', {timeout: 60000}, async () => {
  const browser = await launchBrowser();
  const node = {fnode: randomUUID(), title: 'Lean without a server', module: 'Lib.NoServer', depens: [], blocks: [
    {srctype: 'lean', content: '-- highlighted comment\ntheorem noServer : True := by trivial\n'},
  ]};
  try {
    await fixture(browser, async ({page, url, cli}) => {
      const localLean = [];
      page.on('request', r => { if (/\/lean\.html|\/lean\/(session|check|goals)|\/infoview\//.test(r.url())) localLean.push(r.url()); });
      await page.goto(`${url}/?no-local-lean#ref=${node.fnode}`);
      const block = page.locator('[data-srctype="lean"]');
      await block.locator('.editor-scroll:not(.pending)').waitFor();
      // Same editor as the other source blocks, with Lean 4 syntax colors.
      const colors = await block.locator('.view-line span').evaluateAll(spans => [...new Set(spans.map(el => getComputedStyle(el).color))]);
      assert.ok(colors.length >= 2, 'Lean has syntax colors');
      assert.equal(await page.locator('iframe').count(), 0, 'no native Lean editor frame');
      assert.equal(await page.getByRole('button', {name: 'Start Lean server'}).count(), 0);
      // Two dimensions (plan §7.1): no local check, and not yet submitted to LeanGround.
      const status = page.getByRole('region', {name: 'current node'}).getByLabel(/^Lean: /);
      assert.equal(await status.getAttribute('aria-label'), 'Lean: Not submitted to LeanGround');
      // Editing and saving still work, and write only the block.
      const input = block.getByRole('textbox', {name: /^lean source/});
      await input.press(documentEndKey);
      await page.keyboard.insertText('-- saved without Lean\n');
      await block.getByText('Unsaved', {exact: true}).waitFor();
      await page.keyboard.press('ControlOrMeta+s');
      await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
      assert.match(JSON.parse((await cli('show', node.fnode)).stdout).blocks.find(b => b.srctype === 'lean').content, /saved without Lean/);
      // Project settings no longer offer the local Lake project; LaTeX settings remain.
      await page.getByRole('button', {name: 'Project settings', exact: true}).click();
      const settings = page.getByRole('dialog', {name: 'Project settings'});
      await settings.getByRole('tab', {name: 'LaTeX', exact: true}).waitFor();
      assert.equal(await settings.getByRole('tab', {name: 'Lean', exact: true}).count(), 0);
      assert.equal(localLean.length, 0, `no local Lean requests: ${localLean.join(', ')}`);
    }, [node]);
  } finally { await browser.close(); }
});

// Plan §10 stage 4 in the browser: needs a real LeanGround (LEANGROUND_SERVER_URL,
// LEANGROUND_FACT_TOKEN) whose base key MDC_E2E_BASE_KEY (default 1) contains Nat.
await test('a Lean node is submitted to LeanGround, assembled and written back from the collaboration views', {timeout: 240000, skip: !env.LEANGROUND_SERVER_URL && 'needs a LeanGround server (LEANGROUND_SERVER_URL)'}, async () => {
  const browser = await launchBrowser();
  const make = (title, content, depens = []) => {
    const fnode = randomUUID();
    return {fnode, title, module: `Lib.N_${fnode.replaceAll('-', '')}`, depens, blocks: [{srctype: 'lean', content}]};
  };
  const def = make('triple', 'def triple (n : Nat) : Nat := n + n + n\n');
  const lemma = make('triple_eq', 'theorem triple_eq (n : Nat) : triple n = n + n + n := rfl\n', [def.fnode]);
  const root = make('triple_one', 'theorem triple_one : triple 1 = 3 := by\n  rw [triple_eq]\n', [lemma.fnode, def.fnode]);
  try {
    await fixture(browser, async ({page, url}) => {
      // The proof environment lives in project settings, separate from the Lake project.
      await page.getByRole('button', {name: 'Project settings', exact: true}).click();
      const settings = page.getByRole('dialog', {name: 'Project settings'});
      await settings.getByRole('tab', {name: 'Proof environment'}).click();
      await settings.getByLabel('Base key').fill(env.MDC_E2E_BASE_KEY ?? '1');
      await settings.getByRole('button', {name: 'Save proof environment'}).click();
      await settings.waitFor({state: 'hidden'});

      await page.goto(`${url}/?collab#ref=${root.fnode}`);
      await title(page, 'triple_one');
      const current = page.getByRole('region', {name: 'current node'});
      assert.equal(await current.getByLabel(/^Lean: /).getAttribute('aria-label'), 'Lean: Not submitted to LeanGround');
      await current.getByRole('button', {name: 'Submit to LeanGround'}).click();
      await page.getByRole('dialog', {name: 'Submit to LeanGround'}).getByRole('button', {name: 'Submit', exact: true}).click();

      // Proof overview: the definition is registered, the lemma proved, the root a sketch.
      const collab = page.getByRole('dialog', {name: 'Collaboration'});
      const overview = collab.getByRole('region', {name: 'Proof overview'});
      await overview.getByRole('heading', {name: /triple_one derivable/}).waitFor({timeout: 120000});
      const rows = overview.getByRole('row');
      await rows.filter({hasText: 'triple_eq'}).getByText('submitted').waitFor();
      await rows.filter({hasText: 'triple'}).first().waitFor();
      assert.match(await overview.innerText(), /registered/);

      // Decompositions list the conditional certificate of the root.
      await collab.getByRole('tab', {name: 'Decompositions'}).click();
      await collab.getByRole('region', {name: 'Decompositions'}).getByText(/⇐ triple_eq/).waitFor();

      // Review: assemble a route, then write the certified result back in one batch.
      await collab.getByRole('tab', {name: 'Review'}).click();
      const review = collab.getByRole('region', {name: 'Review'});
      await review.getByRole('button', {name: 'Assemble a route'}).click();
      await review.getByRole('button', {name: 'Accept and write back'}).waitFor({timeout: 120000});
      await review.getByRole('button', {name: 'Accept and write back'}).click();
      await review.getByText(/Writeback .* · committed/).waitFor({timeout: 60000});
      await collab.getByRole('button', {name: 'Close'}).click();

      await page.getByRole('button', {name: 'Refresh database view'}).click();
      await current.getByLabel('Lean: Certified').waitFor();
      const saved = await api(`${url}/api/node/${root.fnode}/view`);
      assert.ok(saved.node.blocks[0].metadata.certification_id, 'the certification is written to the block metadata');
    }, [def, lemma, root]);
  } finally { await browser.close(); }
});

await test('view changes reveal measured editors and loaded pages without intermediate frames', {timeout: 60000}, async () => {
  const browser = await launchBrowser();
  try {
    await fixture(browser, async ({cli, page, url}) => {
      const node = JSON.parse((await cli('show', 'Alpha')).stdout);
      const content = Array.from({length: 100}, (_, i) => `Line ${i + 1}: ${'long text with wrapping '.repeat(i % 4 + 1)}`).join('\n');
      const saved = await fetch(`${url}/api/node/${node.fnode}/block/latex`, {
        method: 'PUT', headers: {'content-type': 'application/json', 'if-match': `"${node.revision}"`}, body: JSON.stringify({content}),
      });
      assert.equal(saved.status, 200);
      await page.reload();
      await page.locator('.line-numbers').nth(2).waitFor();
      await page.waitForFunction(() => document.querySelector('.latex-imports') || document.querySelector('.source-block .view-line'));
      await page.evaluate(() => { window.originalEditor = document.querySelector('.monaco-editor[role=code]'); });
      let release, entered;
      const gate = new Promise(resolve => { release = resolve; });
      const requested = new Promise(resolve => { entered = resolve; });
      await page.route('**/api/graph/full', async route => { entered(); await gate; await route.continue(); });
      await page.getByRole('button', {name: 'Graph', exact: true}).click();
      await requested;
      try {
        await page.waitForFunction(() => document.getAnimations().some(a => a.animationName === 'mdc-hold'));
        assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement, '::view-transition-new(root)').visibility), 'hidden');
      } finally { release(); }
      await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
      await page.unroute('**/api/graph/full');
      // Sample the first frame exposed after each width change, before another measure can mask a bad gutter.
      for (const name of ['Knowledge', 'Graph', 'Knowledge', 'Graph']) {
        await page.evaluate(() => {
          window.firstLayout = new Promise(resolve => {
            let changing = false;
            const sample = () => {
              changing ||= document.documentElement.dataset.vtScope === 'ready';
              if (!changing || document.documentElement.dataset.vtScope) return requestAnimationFrame(sample);
              const editor = document.querySelector('.monaco-editor[role=code]');
              const lines = [...editor.querySelectorAll('.view-line')];
              const gutters = [...editor.querySelectorAll('.line-numbers')].filter(el => el.textContent.trim() && getComputedStyle(el).visibility !== 'hidden');
              resolve(gutters.map(g => ({number: g.textContent, difference: Math.min(...lines.map(line => Math.abs(g.getBoundingClientRect().top - line.getBoundingClientRect().top)))})));
            };
            requestAnimationFrame(sample);
          });
        });
        await page.getByRole('button', {name, exact: true}).click();
        const rows = await page.evaluate(() => window.firstLayout);
        assert.ok(rows.length > 3);
        assert.ok(rows.every(row => row.difference < 1), JSON.stringify(rows));
      }
      assert.equal(await page.evaluate(() => window.originalEditor === document.querySelector('.monaco-editor[role=code]')), true);
      // Cross-document transitions also hold the old view while project and node data are pending.
      // Reduced motion still needs readiness gating, without a fade animation.
      await page.emulateMedia({reducedMotion: 'reduce'});
      const held = async (pattern, navigate) => {
        let release, entered;
        const gate = new Promise(resolve => { release = resolve; });
        const started = new Promise(resolve => { entered = resolve; });
        await page.route(pattern, async route => { entered(); await gate; await route.continue(); });
        try {
          await navigate();
          await Promise.race([started, new Promise((_, reject) => setTimeout(() => reject(new Error(`request not observed: ${pattern}`)), 5000))]);
          await page.waitForFunction(() => document.getAnimations().some(a => a.animationName === 'mdc-hold'));
          assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement, '::view-transition-new(root)').visibility), 'hidden');
        } finally { release(); }
        await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
        await page.unroute(pattern);
      };
      await held('**/api/projects', () => page.getByRole('link', {name: 'All projects', exact: true}).click());
      await page.getByRole('heading', {name: 'Projects', exact: true}).waitFor();
      const project = new URL(url).pathname.replace(/^\/p\//, '').replace(/\/$/, '');
      await held('**/api/node/*/view', () => page.getByRole('link', {name: `Open ${project}`, exact: true}).click());
      assert.equal(await page.locator('.editor-loading').count(), 0);
      await page.locator('.line-numbers').nth(2).waitFor();
      // A failed destination must reveal its error/retry UI instead of freezing the old page forever.
      await page.route('**/api/projects', route => route.fulfill({status: 503, contentType: 'application/json', body: JSON.stringify({error: 'fixture: status unavailable'})}));
      await page.getByRole('link', {name: 'All projects', exact: true}).click();
      await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
      await page.getByRole('alert').filter({hasText: 'fixture: status unavailable'}).waitFor();

    });
  } finally { await browser.close(); }
});
