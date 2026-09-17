import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { chromium, webkit } from "playwright";
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
    page.on("pageerror", (error) => errors.push((error.stack || error.message).replace(/^Unhandled Promise Rejection: /, "")));
    page.on("dialog", (dialog) => void dialog.accept());
    try {
      await page.goto(`${server.url}/#ref=${bundle.nodes[0].fnode}`);
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

await test('project directory creates, forks, starts, stops and deletes branches', {timeout: 90000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  try {
    await fixture(browser, async ({page, url}) => {
      const base = new URL(url).origin, main = new URL(url).pathname.slice(3);
      const database = main.split('/')[0], fork = `${database}/draft`;
      const initialized = `mdce2einit${randomUUID().replaceAll('-', '')}`;
      const row = name => page.locator(`[data-project="${name}"]`);
      await page.goto(base);
      await page.getByRole('textbox', {name: 'Search projects and branches'}).fill(database);
      await row(main).getByText('Running', {exact: true}).waitFor();
      assert.equal(await page.getByText('MAIN', {exact: true}).count(), 0);
      assert.equal(await page.getByText('MATHEMATICAL WORKSPACE', {exact: true}).count(), 0);
      assert.match(await row(main).locator('.counts').innerText(), /3.*nodes/);
      assert.match(await row(main).locator('.counts').innerText(), /1.*edge/);
      assert.equal(await row(main).getByRole('button', {name: `Delete ${main}`, exact: true}).isDisabled(), true);
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
      await row(fork).getByText('Stopped', {exact: true}).waitFor();
      assert.equal((await row(fork).locator('.counts').innerText()).trim(), '');
      const positions = await Promise.all([main, fork].map(name => row(name).locator('.counts').boundingBox()));
      assert.equal(positions[0].x, positions[1].x); assert.equal(positions[0].width, positions[1].width);
      await branch(fork, 'copy');
      await row(fork).getByRole('button', {name: `Start ${fork}`, exact: true}).click();
      await row(fork).getByText('Running', {exact: true}).waitFor();
      assert.match(await row(fork).locator('.counts').innerText(), /3.*nodes/);
      await row(fork).getByRole('button', {name: `Stop ${fork}`, exact: true}).click();
      await row(fork).getByText('Stopped', {exact: true}).waitFor();
      assert.equal((await row(fork).locator('.counts').innerText()).trim(), '');
      for (const theme of ['dark', 'light']) {
        if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) await page.getByRole('button', {name: 'Toggle theme'}).click();
        for (const width of [1440, 750, 420]) {
          await page.setViewportSize({width, height: 900});
          assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth || document.querySelector('.directory').scrollWidth > innerWidth), false);
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
        await row(`${initialized}/main`).getByText('Stopped', {exact: true}).waitFor();
        await row(`${initialized}/main`).getByRole('button', {name: `Start ${initialized}/main`, exact: true}).click();
        await row(`${initialized}/main`).getByText('Running', {exact: true}).waitFor();
        assert.match(await row(`${initialized}/main`).locator('.counts').innerText(), /0.*nodes/);
      } finally {
        await fetch(`${base}/api/projects`, {method: 'POST', headers: {'content-type': 'application/json', origin: base}, body: JSON.stringify({action: 'stop', project: `${initialized}/main`})});
        const removed = await fetch(`${env.MDC_TERMINUS_URL ?? 'http://127.0.0.1:6363'}/api/db/admin/${initialized}`, {
          method: 'DELETE', headers: {Authorization: `Basic ${Buffer.from(`${env.MDC_TERMINUS_USER ?? 'admin'}:${env.MDC_TERMINUS_PASSWORD}`).toString('base64')}`},
        });
        assert.ok(removed.ok || removed.status === 404);
      }
      await page.route('**/api/projects', route => route.fulfill({json: {server: {running: true}, projects: {}}}));
      await page.goto(base);
      await page.getByRole('heading', {name: 'No projects yet'}).waitFor();
      assert.equal(await page.getByText(/Create a project with|Open a running branch/).count(), 0);
      assert.equal(await page.getByRole('button', {name: 'Init project', exact: true}).isEnabled(), true);
    });
  } finally { await browser.close(); }
});

await test("large relation lists filter and retain natural card heights", { timeout: 90000 }, async () => {
  const browser = process.env.MDC_E2E_BROWSER === "webkit" ? await webkit.launch() : await chromium.launch({ headless: true, channel: "chromium" });
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

await test("browser with the real MathDoc backend", { timeout: 240000 }, async (suite) => {
  const browser = process.env.MDC_E2E_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ headless: true, channel: "chromium" });
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
          let opened = 0, closed = 0, currentSocket;
          page.on("websocket", socket => { currentSocket = socket; opened++; socket.on("close", () => closed++); });
          await page.getByRole("button", { name: /Search nodes/ }).click();
          await page.getByPlaceholder("Search by title or fnode…").fill(node.title);
          await page.getByRole("dialog").getByRole("button", { name: new RegExp(node.title) }).click();
          await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
          await page.getByText("Lean editor ready", { exact: true }).waitFor();
          const otherPage = await page.context().newPage();
          const otherConnection = otherPage.waitForEvent("websocket");
          await otherPage.goto(`${started.url}#ref=${node.fnode}`);
          await otherPage.getByRole("button", { name: "Start Lean server", exact: true }).click();
          await otherPage.getByText("Lean editor ready", { exact: true }).waitFor();
          const otherSocket = await otherConnection;
          await Promise.all([once(otherSocket, "close", { signal: AbortSignal.timeout(10000) }), manage("stop", agent)]);
          agentRunning = false;
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
          await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
          await page.getByText("Lean editor ready", { exact: true }).waitFor();
          await Promise.all([once(currentSocket, "close", { signal: AbortSignal.timeout(10000) }), manage("stop")]);
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
        await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
        await page.frameLocator('iframe[title="Lean source and Infoview"]').locator('#infoview-pending').waitFor();
        await page.locator('.native-editor:not(.pending)').waitFor();
        await page.locator('[data-srctype="latex"] .monaco-editor').waitFor();
        assert.deepEqual(await center(page).locator('.source-block:not(.hidden)').evaluateAll(
          blocks => blocks.map(block => block.dataset.srctype),
        ), ["text", "latex", "lean", "rocq"]);
        for (const name of ["Graph", "Knowledge"]) {
          await page.getByRole("button", { name, exact: true }).click();
          await page.getByRole("button", { name, exact: true }).and(page.locator('[aria-pressed="true"]')).waitFor({ timeout: 1000 });
        }
        releaseSession();
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        await frame.locator(".monaco-editor").waitFor();
        const input = frame.getByRole("textbox", { name: /Editor content/ });
        await frame.locator(".view-line").getByText("constructor", { exact: true }).click();
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
        await center(page).getByLabel("Lean: Verified", { exact: true }).waitFor();
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
        await center(page).getByLabel("Lean: Verified", { exact: true }).waitFor();
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
        await frame.locator(".view-line").getByText("constructor", { exact: true }).waitFor();
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
        await frame.locator(".view-line").getByText("constructor", { exact: true }).waitFor();
        assert.equal(await page.getByText("Unsaved", { exact: true }).count(), 0);
        for (let i = 0; i < 3; i++) {
          const old = await page.locator(".lean-block").getAttribute("data-session");
          const rechecked = socket.waitForEvent("framesent", {predicate: ({payload}) => JSON.parse(String(payload)).method === "textDocument/didOpen"});
          await page.getByRole("button", { name: "Recheck Lean", exact: true }).click();
          await rechecked;
          assert.equal((await fetch(`${url}/api/lean/session/${old}`)).status, 200, "recheck must retain the LSP session");
          assert.equal(messages.filter(m => m.method === "initialize").length, 1);
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
        await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
        await page.getByText(/Lean (?:connection closed;|failed to initialize:)/).first().waitFor({ timeout: 10000 });
        await page.waitForFunction(() => /Lean (?:connection closed;|failed to initialize:)/.test(document.body.innerText) && !document.body.innerText.includes("Reconnecting Lean…"));
        assert.equal(attempts, 2, "only one automatic reconnect is allowed");
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        await frame.getByRole("textbox", { name: /Editor content/ }).press(await page.evaluate(() => /Mac/.test(navigator.platform)) ? "Meta+ArrowDown" : "Control+End");
        await page.keyboard.insertText("-- draft after failed startup\n");
        await page.getByText("Unsaved", { exact: true }).waitFor();
        fail = false;
        await page.getByRole("button", { name: "Stop Lean server", exact: true }).click();
        await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await frame.getByText("-- draft after failed startup", { exact: true }).waitFor();
        assert.doesNotMatch(JSON.parse((await cli("show", "Alpha")).stdout).blocks[0].content, /draft after failed startup/);
      }, [], [
        // Upstream emits shutdown rejections when initialize is interrupted.
        // Only this fault-injection fixture allows them; recovery is asserted above.
        /^Error: Client is not running and can't be stopped\. It's current state is: starting\n/,
        /^(?:\w+: )?Pending response rejected since connection got disposed(?:\n|$)/,
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
        await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
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
        const sessionPath = () => page.locator(".lean-block").getAttribute("data-session");
        const oldPath = await sessionPath();
        const oldId = oldPath;
        const workers = async () => (await run("ps", ["-axo", "pid=,command="])).stdout.split("\n")
          .filter(line => line.includes("--worker ") && line.includes(root)).map(line => Number(line.trim().split(/\s+/)[0]));
        const oldWorkers = await workers();
        assert.ok(oldWorkers.length > 0, "the session must have native file workers");
        // Keep the current selection unanswered. Its 30-second deadline must
        // reconnect, retire native workers, and restore the parent-owned draft.
        await page.waitForFunction(old => { const id = document.querySelector(".lean-block")?.getAttribute("data-session"); return id && id !== old; }, oldPath, { timeout: 35000 });
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
        await page.getByRole("button", { name: "Start Lean server", exact: true }).click();
        await page.getByText("Lean editor ready", { exact: true }).waitFor();
        await center(page).getByLabel("Lean: Verified", { exact: true }).waitFor();
        const leanStatus = async id => (await (await fetch(`${url}/api/node/${id}/view`)).json()).node.formalization.lean;
        assert.equal(await leanStatus(dep.fnode), "verified", "the editor's compiled imports must certify dependencies too");
        const dependencyCard = page.locator(`.card[data-fnode="${dep.fnode}"]`);
        await dependencyCard.getByLabel("Lean: Verified", { exact: true }).waitFor();
        await dependencyCard.getByLabel("Rocq: No code", { exact: true }).waitFor();
        const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
        assert.equal(await frame.locator(".squiggly-error").count(), 0);
        await put(dep.fnode, "def anchor : Nat := 2\n");
        assert.equal(await leanStatus(target.fnode), "unverified", "changing an import must invalidate the dependent certificate");
        await select("Alpha");
        await select("Lean Dependent");
        await frame.locator(".squiggly-error").first().waitFor();
        await page.getByText("Lean errors", { exact: false }).waitFor();
        assert.equal(await center(page).getByLabel("Lean: Verified", { exact: true }).count(), 0);
        assert.equal(await leanStatus(target.fnode), "unverified");
        assert.equal(sent.filter(m => m.method === "initialize").length, 1);
        assert.ok(sent.some(m => m.method === "textDocument/didClose"), "a changed dependency must invalidate the worker environment");
      }));
  } finally { await browser.close(); }
});

await test('Monaco source blocks retain highlighting, edits and undo across layout changes', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
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
        await input.press(await page.evaluate(() => /Mac/.test(navigator.platform)) ? 'Meta+ArrowDown' : 'Control+End');
        await page.keyboard.insertText('draft');
        await block.getByText('Unsaved', {exact: true}).waitFor();
        await page.getByRole('button', {name: 'Graph', exact: true}).click();
        await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
        await input.press('ControlOrMeta+z');
        await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
        assert.equal(JSON.parse((await cli('show', node.fnode)).stdout).blocks.find(b => b.srctype === srctype).content, content);
        await page.getByRole('button', {name: 'Knowledge', exact: true}).click();
        await page.waitForFunction(() => !document.documentElement.dataset.vtScope);
      }
      await page.getByRole('button', {name: 'Switch to dark mode'}).click();
      for (const {srctype} of node.blocks) await page.locator(`[data-srctype="${srctype}"] .monaco-editor.vs-dark`).waitFor();
      assert.equal(sessions, 0, 'ordinary editing never starts a Lean server');
    }, [node]);
  } finally { await browser.close(); }
});

await test('LaTeX macros, scoped completion, citations and draft previews', {timeout: 90000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
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
      await writeFile(bib, '@article{paper,title={A paper},author={Author, A.},journal={Journal},year={2020}}' +
        Array.from({length: 80}, (_, i) => `@article{zextra${i},title={Another publication ${i}},author={Writer, B.},year={2021}}`).join('\n'));
      await page.getByRole('button', {name: 'Save LaTeX project', exact: true}).waitFor();
      await page.getByLabel('Class or preamble').setInputFiles(preamble);
      await page.getByLabel('Bibliography', {exact: true}).setInputFiles(bib);
      await page.getByRole('button', {name: 'Save LaTeX project', exact: true}).click();
      await page.getByRole('dialog').waitFor({state: 'hidden'});
      const settings = JSON.parse((await cli('project', 'latex', 'show')).stdout).project;
      assert.match(settings.preamble, /newcommand/);
      assert.match(settings.bibliography, /A paper/);
      const source = String.raw`\section{Introduction}\label{intro}By \nameref{thm:b}, see \cite{paper}. $\cA$\begin{items}\item One\end{items}`;
      await put('Alpha', source);
      await put('Beta', String.raw`\begin{thm}[Named result]\label{thm:b}$\cA$ exists.\end{thm}`);
      await put('Gamma', String.raw`\section{Private result}\label{private}`);
      const externalName = number => `${ids.Beta.slice(0, 8)}::Theorem ${number}`;
      await page.reload();
      const block = page.locator('article[data-srctype="latex"]');
      await block.getByText('1 imported dependencies', {exact: false}).waitFor();
      await block.locator('.latex-imports summary').click();
      await block.getByRole('button', {name: 'Render LaTeX preview'}).click();
      assert.equal(await block.locator('.latex-imports summary').innerText(), '1 imported dependencies');
      await block.locator('.latex-imports li').getByText('Beta', {exact: true}).waitFor();
      await block.getByRole('link', {name: externalName(1), exact: true}).waitFor();
      await block.locator('.katex').first().waitFor();
      assert.equal(await block.locator('.katex-error').count(), 0);
      assert.match(await block.locator('.latex-preview').innerText(), /A paper/);
      await block.getByRole('link', {name: externalName(1), exact: true}).click();
      await title(page, 'Beta');
      await block.locator('.latex-preview .latex-statement').waitFor();
      assert.equal(await block.locator('[id="latex-thm%3Ab"]').count(), 1);
      assert.equal(await block.locator('.latex-statement-title').innerText(), 'Theorem 1 (Named result)');
      const headingGap = await block.locator('.latex-statement').evaluate(element => {
        const title = element.querySelector('.latex-statement-title').getClientRects()[0];
        const paragraph = element.querySelector('p').getClientRects()[0];
        return Math.abs(title.y - paragraph.y);
      });
      assert.ok(headingGap < 4, 'theorem heading and first paragraph must share a line');
      await page.goBack();
      await title(page, 'Alpha');
      const input = block.getByRole('textbox', {name: /^latex source/});
      const fill = async value => { await input.press('ControlOrMeta+A'); await page.keyboard.insertText(value); };
      await fill('Use \\nameref{');
      await input.press('Control+Space');
      await page.getByRole('option').filter({hasText: 'Named result'}).click();
      await page.waitForFunction(() => /Use.*nameref\{thm:b/.test(document.querySelector('[data-srctype="latex"] .view-lines').innerText));
      assert.match(await block.locator('.view-lines').innerText(), /Use.*nameref\{thm:b/);
      assert.equal(await page.getByRole('option').filter({hasText: 'Private result'}).count(), 0);
      for (const theme of ['dark', 'light']) {
        await page.getByRole('button', {name: `Switch to ${theme} mode`}).click();
        await fill('% filler\n'.repeat(7) + '\\cite{');
        await input.press('Control+Space');
        const option = page.getByRole('option').filter({hasText: 'A paper'});
        await option.waitFor();
        const popup = page.locator('.mdc-editor-widgets .suggest-widget:visible');
        assert.equal(await popup.count(), 1, 'completion escapes the source block clipping context');
        const geometry = await option.evaluate(el => {
          const add = document.querySelector('.add-btn').getBoundingClientRect();
          const widget = el.closest('.suggest-widget');
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
        assert.equal(await popup.evaluate(el => [el, ...el.querySelectorAll('.monaco-scrollable-element, .monaco-list-rows')].every(surface => {
          surface.scrollTop = 100;
          return surface.scrollTop === 0;
        })), true, 'native scrolling cannot reveal rows outside Monaco\'s rendered window');
        await page.mouse.move(popupBox.x + 100, popupBox.y + 70);
        await popup.evaluate(el => {
          window.completionFrames = []; window.recordCompletion = true;
          const sample = () => {
            const box = el.getBoundingClientRect(), list = el.querySelector('.monaco-list-rows');
            window.completionFrames.push({x: box.x, y: box.y, height: box.height, shown: getComputedStyle(el).visibility,
              list: parseFloat(list.style.top), pane: document.querySelector('.blocks').scrollTop,
              hit: el.contains(document.elementFromPoint(box.x + 100, box.y + 70))});
            if (window.recordCompletion) requestAnimationFrame(sample);
          }; requestAnimationFrame(sample);
        });
        for (const direction of [1, -1]) for (let step = 0; step < 10; step++) {
          await page.mouse.wheel(0, direction * 30);
          await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
        }
        const frames = await page.evaluate(() => {window.recordCompletion = false; return window.completionFrames;});
        assert.ok(frames.some(frame => frame.list < -100), 'scroll through the virtualized citation candidates');
        assert.ok(frames.every(frame => frame.x === popupBox.x && frame.y === popupBox.y && frame.height === popupBox.height && frame.shown === 'visible' && frame.hit), 'completion geometry and hit testing stay stable through every scroll frame');
        assert.equal(new Set(frames.map(frame => frame.pane)).size, 1, 'candidate scrolling never moves the page underneath');
        await page.screenshot({path: resolve(tmpdir(), `mdc-completion-${theme}-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
        await option.click();
        await page.waitForFunction(() => /cite\{paper/.test(document.querySelector('[data-srctype="latex"] .view-lines').innerText));
        assert.match(await block.locator('.view-lines').innerText(), /cite\{paper/);
      }
      const draft = String.raw`\section{Draft title}\label{new}By \nameref{thm:b}, see \cite{paper}. $\cA$`;
      await fill(draft);
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

await test('Lean first paint waits for syntax highlighting without a server', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const node = {fnode: randomUUID(), title: 'Highlighted first paint', module: 'Lib.FirstPaint', depens: [], blocks: [
    {srctype: 'lean', content: '-- highlighted comment\ntheorem firstPaint : True := by trivial\n'},
  ]};
  try {
    await fixture(browser, async ({page, url}) => {
      let release, requested, sessions = 0;
      const gate = new Promise(resolve => { release = resolve; });
      const grammar = new Promise(resolve => { requested = resolve; });
      await page.route(/\/assets\/lean4-[^/]+\.json$/, async route => {
        requested(); await gate; await route.continue();
      });
      page.on('request', r => { if (r.method() === 'POST' && r.url().endsWith('/lean/session')) sessions++; });
      await page.addInitScript(() => {
        const observe = () => {
          const frame = document.querySelector('iframe[title="Lean source and Infoview"]');
          const lines = frame?.contentDocument?.querySelectorAll('.view-line span');
          if (frame && getComputedStyle(frame).visibility === 'visible' && lines?.length) {
            window.firstLeanPaint = [...new Set([...lines].map(el => frame.contentWindow.getComputedStyle(el).color))];
          } else requestAnimationFrame(observe);
        };
        requestAnimationFrame(observe);
      });
      try {
        await page.goto(`${url}/?first-paint#ref=${node.fnode}`);
        await grammar;
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        assert.equal(await page.getByText('Loading Lean editor…', {exact: true}).isVisible(), false);
        assert.equal(await page.getByLabel('Lean source while editor loads').isVisible(), false);
        assert.equal(await page.locator('iframe[title="Lean source and Infoview"]').isVisible(), false, 'hold the editor until its grammar is loaded');
      } finally { release(); }
      await page.locator('.native-editor:not(.pending)').waitFor();
      await page.waitForFunction(() => window.firstLeanPaint);
      assert.ok((await page.evaluate(() => window.firstLeanPaint)).length >= 2, 'the first visible frame must already contain syntax colors');
      await page.frameLocator('iframe[title="Lean source and Infoview"]').getByText('firstPaint', {exact: true}).waitFor();
      assert.equal(sessions, 0);
    }, [node]);
  } finally { await browser.close(); }
});

await test('Lean hover stays above the active Infoview', {timeout: 45000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const node = {fnode: randomUUID(), title: 'Hover', module: 'Lib.Hover', depens: [], blocks: [
    {srctype: 'lean', content: '/-- Documentation with enough text to extend across the narrow source pane into the active Infoview. -/\ndef hoverTargetWithALongName : Nat := 42\n#check hoverTargetWithALongName\n'},
  ]};
  try {
    await fixture(browser, async ({page, url}) => {
      await page.goto(`${url}/?hover#ref=${node.fnode}`);
      await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      await frame.frameLocator('#infoview iframe').getByText(/All Messages/).first().waitFor();
      await frame.locator('.view-line').getByText('hoverTargetWithALongName', {exact: true}).last().hover();
      const hover = frame.locator('.monaco-hover:visible');
      await hover.waitFor();
      const overlap = await hover.evaluate(el => {
        const box = el.getBoundingClientRect(), info = document.getElementById('infoview').getBoundingClientRect();
        const x = Math.max(box.left, info.left) + 5, y = box.top + 10;
        return {crosses: x < box.right, onTop: el.contains(document.elementFromPoint(x, y))};
      });
      assert.equal(overlap.crosses, true, 'exercise a tooltip crossing into Infoview');
      assert.equal(overlap.onTop, true, 'the source tooltip paints and receives input above Infoview');
      await page.screenshot({path: resolve(tmpdir(), `mdc-lean-hover-${process.env.MDC_E2E_BROWSER ?? 'chromium'}.png`)});
    }, [node]);
  } finally { await browser.close(); }
});

await test('wrapped Lean sources reach the last line before and after server startup', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const node = {fnode: randomUUID(), title: 'Wrapped Lean', module: 'Lib.Wrapped', depens: [], blocks: [
    {srctype: 'lean', content: Array.from({length: 60}, (_, i) => `-- ${i} ${'Long wrapped Lean source. '.repeat(9)}`).join('\n') + '\n-- DOCUMENT END'},
  ]};
  try {
    await fixture(browser, async ({page, url}) => {
      await page.goto(`${url}/?wrapped#ref=${node.fnode}`);
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      for (const running of [false, true]) {
        if (running) {
          await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
          await page.getByText('Lean editor ready', {exact: true}).waitFor();
        }
        await page.locator('.native-editor:not(.pending)').waitFor();
        for (const view of ['Knowledge', 'Graph', 'Knowledge']) {
          await page.getByRole('button', {name: view, exact: true}).click();
          const scroll = frame.locator('#editor-scroll');
          await scroll.evaluate(el => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
          const size = await scroll.evaluate(el => {
            el.scrollTop = el.scrollHeight;
            return {height: el.clientHeight, content: el.scrollHeight, viewport: innerHeight, width: el.clientWidth};
          });
          console.log('Wrapped source viewport:', running, view, size);
          assert.ok(size.height <= size.viewport, 'the native scroller must fit inside the iframe');
          const end = frame.getByText('-- DOCUMENT END', {exact: true});
          await end.waitFor({timeout: 3000});
          assert.ok(await end.evaluate(el => {
            const r = el.getBoundingClientRect();
            const frame = window.frameElement;
            return r.top >= 0 && r.bottom <= innerHeight + 1 &&
              r.bottom + frame.getBoundingClientRect().top <= frame.closest('article').getBoundingClientRect().bottom;
          }), 'the final source line must fit inside both the iframe and the clipped code block');
        }
      }
    }, [node]);
  } finally { await browser.close(); }
});

await test('stopping an unfinished Lean check clears progress and Infoview connections', {timeout: 90000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const node = {fnode: randomUUID(), title: 'Interruptible', module: 'Lib.Interruptible', depens: [], blocks: [
    {srctype: 'lean', content: '#eval IO.sleep 60000\nexample : True := by trivial\n'},
  ]};
  try {
    await fixture(browser, async ({page, url, root}) => {
      await page.goto(`${url}/?stop#ref=${node.fnode}`);
      const block = page.locator('.lean-block');
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      const input = frame.getByRole('textbox', {name: /Editor content/});
      const gutter = frame.locator('.glyph-margin-widgets [class*="ced-"]');
      const workers = async () => (await run('ps', ['-axo', 'pid=,command='])).stdout.split('\n').filter(line => line.includes(root) && /--worker|--server/.test(line));
      for (let attempt = 0; attempt < 2; attempt++) {
        await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
        await gutter.first().waitFor();
        const info = frame.frameLocator('#infoview iframe');
        await info.getByText('All Messages', {exact: true}).waitFor();
        assert.doesNotMatch(await info.locator('body').innerText(), /No connection to Lean|Error updating/);
        assert.ok((await workers()).length > 0, 'a native Lean process is checking the unfinished source');
        const instance = await frame.locator('.monaco-editor').elementHandle();
        const stopped = page.waitForResponse(r => r.request().method() === 'DELETE' && /\/lean\/session\//.test(r.url()));
        const start = Date.now();
        await block.getByRole('button', {name: 'Stop Lean server', exact: true}).click();
        assert.equal((await stopped).status(), 204);
        await gutter.first().waitFor({state: 'hidden'});
        assert.ok(Date.now() - start < 5000, 'stop does not wait for the sixty-second check');
        assert.equal(await frame.locator('#infoview iframe').count(), 0, 'dispose the old Infoview RPC connection');
        assert.equal(await instance.evaluate(el => el.isConnected), true, 'preserve the syntax editor');
        assert.deepEqual(await workers(), [], 'stop reaps the native server and its workers');
      }
      await input.press('ControlOrMeta+A');
      await page.keyboard.insertText('example : True := by trivial\n');
      await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      await frame.frameLocator('#infoview iframe').getByText('All Messages', {exact: true}).waitFor();
      assert.doesNotMatch(await frame.frameLocator('#infoview iframe').locator('body').innerText(), /No connection to Lean|Error updating/);
    }, [node]);
  } finally { await browser.close(); }
});

await test('Lean session state survives navigation through nodes without Lean', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const nodes = ['First', 'Second'].map(title => ({fnode: randomUUID(), title, module: `Lib.${title}`, depens: [], blocks: [
    {srctype: 'lean', content: 'example : True := by trivial\n'},
  ]}));
  try {
    await fixture(browser, async ({page, url}) => {
      let sessions = 0, sockets = 0, release, allocated;
      page.on('request', r => { if (r.method() === 'POST' && r.url().endsWith('/lean/session')) sessions++; });
      page.on('websocket', () => sockets++);
      const select = async name => {
        await page.getByRole('button', {name: /Search nodes/}).click();
        await page.getByPlaceholder('Search by title or fnode…').fill(name);
        await page.getByRole('dialog').getByRole('button', {name: new RegExp(name)}).click();
        await title(page, name);
      };
      const gate = new Promise(resolve => { release = resolve; });
      const allocation = new Promise(resolve => { allocated = resolve; });
      await page.route(/\/lean\/session$/, async route => {
        const response = await route.fetch();
        allocated((await response.json()).id);
        await gate;
        await route.fulfill({response});
      });
      await select('First');
      await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      const id = await allocation;
      await select('Alpha');
      release();
      await page.waitForFunction(id => document.querySelector('.lean-block').dataset.session === id, id);
      for (const name of ['Second', 'Alpha', 'First']) {
        await select(name);
        if (name !== 'Alpha') await page.getByText('Lean editor ready', {exact: true}).waitFor();
        assert.equal(await page.locator('.lean-block').getAttribute('data-session'), id);
        assert.equal((await fetch(`${url}/api/lean/session/${id}`)).status, 200);
      }
      assert.equal(sessions, 1); assert.equal(sockets, 1);
      const stopped = page.waitForResponse(r => r.request().method() === 'DELETE' && r.url().endsWith(`/lean/session/${id}`));
      await page.getByRole('button', {name: 'Stop Lean server', exact: true}).click();
      assert.equal((await stopped).status(), 204);
      await select('Alpha'); await select('Second');
      await page.getByRole('button', {name: 'Start Lean server', exact: true}).waitFor();
      assert.equal(await page.locator('.lean-block').getAttribute('data-session'), null);
      assert.equal(sessions, 1); assert.equal(sockets, 1);
    }, nodes);
  } finally { await browser.close(); }
});

await test('Lean browsing stays offline until explicitly started and preserves static drafts', {timeout: 90000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const nodes = ['First', 'Second'].map((title, i) => ({fnode: randomUUID(), title, module: `Lib.Offline${i}`, depens: [], blocks: [
    {srctype: 'lean', content: `-- A highlighted comment\ntheorem offline${i} : True := by trivial\n`},
  ]}));
  try {
    await fixture(browser, async ({page, url, cli, root}) => {
      let sessions = 0, sockets = 0, socket;
      page.on('request', request => { if (request.method() === 'POST' && request.url().endsWith('/lean/session')) sessions++; });
      page.on('websocket', ws => { sockets++; socket = ws; });
      await page.goto(`${url}/?offline#ref=${nodes[0].fnode}`);
      const block = page.locator('.lean-block');
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      const input = frame.getByRole('textbox', {name: /Editor content/});
      const styled = async () => {
        const keyword = frame.locator('.view-line').getByText('theorem', {exact: true});
        await keyword.waitFor();
        return keyword.evaluate(() => {
          // Semantic tokens may replace a span between locator resolution and
          // evaluation. Read the current token and its style in one DOM turn.
          const el = [...document.querySelectorAll('.view-line span')].find(el => el.textContent === 'theorem');
          const style = getComputedStyle(el);
          return {color: style.color, font: style.fontFamily, size: style.fontSize};
        });
      };
      await page.locator('.native-editor:not(.pending)').waitFor();
      const light = await styled();
      let persistentEditor;
      const sameEditor = async () => {
        assert.equal(await persistentEditor.evaluate(el => el.isConnected), true, 'start/stop retain the editor DOM and iframe');
        assert.equal(await block.locator('.native-editor.pending').count(), 0, 'start/stop never hide the source');
        assert.equal(await page.getByText('Starting Lean editor…', {exact: true}).count(), 0);
      };
      await page.getByRole('button', {name: 'Switch to dark mode'}).click();
      await frame.locator('.monaco-editor.vs-dark').waitFor();
      const dark = await styled();
      assert.notEqual(light.color, dark.color, 'offline syntax responds to the theme');
      assert.equal(await frame.locator('#infoview').isVisible(), false);
      await input.press('ControlOrMeta+End');
      await page.keyboard.insertText('-- saved without a server\n');
      await block.getByRole('button', {name: 'Save', exact: true}).click();
      await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
      assert.match(JSON.parse((await cli('show', nodes[0].fnode)).stdout).blocks[0].content, /saved without a server/);
      const select = async name => {
        await page.getByRole('button', {name: /Search nodes/}).click();
        await page.getByPlaceholder('Search by title or fnode…').fill(name);
        await page.getByRole('dialog').getByRole('button', {name: new RegExp(name)}).click();
        await title(page, name);
        await page.locator('.native-editor:not(.pending)').waitFor();
      };
      await select('Second');
      await frame.getByText('offline1', {exact: true}).waitFor();
      await select('First');
      await frame.getByText('-- saved without a server', {exact: true}).waitFor();
      assert.equal(sessions, 0); assert.equal(sockets, 0);
      const processes = (await run('ps', ['-axo', 'command='])).stdout.split('\n');
      assert.equal(processes.filter(line => line.includes(root) && /--worker|lake serve/.test(line)).length, 0);
      // Saving offline must update the revision used by the next attachment.
      await input.press('ControlOrMeta+End');
      await page.keyboard.insertText('-- offline revision\n');
      await block.getByRole('button', {name: 'Save', exact: true}).click();
      await block.getByText('Unsaved', {exact: true}).waitFor({state: 'hidden'});
      persistentEditor = await frame.locator('.monaco-editor').elementHandle();
      await input.press('ControlOrMeta+End');
      await page.keyboard.insertText('-- draft before startup\n');
      await block.getByText('Unsaved', {exact: true}).waitFor();
      await block.getByRole('button', {name: 'Collapse block'}).click();
      await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      assert.equal(sessions, 1); assert.equal(sockets, 1);
      assert.equal(await block.getByRole('button', {name: 'Expand block'}).count(), 1);
      await block.getByRole('button', {name: 'Expand block'}).click();
      await frame.getByText('-- draft before startup', {exact: true}).waitFor();
      await sameEditor();
      assert.deepEqual(await styled(), dark, 'the same grammar, theme and font apply after startup');
      await input.press('ControlOrMeta+z');
      await frame.getByText('-- draft before startup', {exact: true}).waitFor({state: 'hidden'});
      await input.press('ControlOrMeta+Shift+z');
      await frame.getByText('-- draft before startup', {exact: true}).waitFor();
      assert.doesNotMatch(JSON.parse((await cli('show', nodes[0].fnode)).stdout).blocks[0].content, /draft before startup/);
      const rechecked = socket.waitForEvent('framesent', {predicate: ({payload}) => JSON.parse(String(payload)).method === 'textDocument/didOpen'});
      await block.getByRole('button', {name: 'Recheck Lean', exact: true}).click();
      await rechecked;
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      await frame.getByText('-- draft before startup', {exact: true}).waitFor();
      assert.equal(sessions, 1);
      assert.equal(sockets, 1);
      await select('Second');
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      assert.equal(sessions, 1, 'explicitly started sessions are reused across nodes');
      // Stop only this page's session, preserving drafts and another page's worker.
      const other = await page.context().newPage();
      await other.goto(`${url}/?other#ref=${nodes[0].fnode}`);
      await other.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      await other.getByText('Lean editor ready', {exact: true}).waitFor();
      const sessionId = p => p.locator(".lean-block").getAttribute("data-session");
      const oldId = await sessionId(page), otherId = await sessionId(other);
      persistentEditor = await frame.locator('.monaco-editor').elementHandle();
      await input.press('ControlOrMeta+End');
      await page.keyboard.insertText('-- draft survives stop\n');
      const closed = socket.waitForEvent('close');
      const stoppedSession = page.waitForResponse(r => r.request().method() === 'DELETE' && r.url().endsWith(`/lean/session/${oldId}`));
      await block.getByRole('button', {name: 'Stop Lean server', exact: true}).click();
      await closed;
      assert.equal((await stoppedSession).status(), 204);
      await frame.getByText('-- draft survives stop', {exact: true}).waitFor();
      await sameEditor();
      assert.equal(await frame.locator('#infoview').isVisible(), false);
      await input.press('ControlOrMeta+z');
      await frame.getByText('-- draft survives stop', {exact: true}).waitFor({state: 'hidden'});
      await input.press('ControlOrMeta+Shift+z');
      await frame.getByText('-- draft survives stop', {exact: true}).waitFor();
      assert.equal(await sessionId(page), null);
      assert.equal((await fetch(`${url}/api/lean/session/${oldId}`)).status, 404);
      assert.equal((await fetch(`${url}/api/lean/session/${otherId}`)).status, 200);
      const otherFrame = other.frameLocator('iframe[title="Lean source and Infoview"]');
      await otherFrame.getByRole('textbox', {name: /Editor content/}).press('ControlOrMeta+A');
      await other.keyboard.insertText('example : True := by exact 42');
      await otherFrame.locator('.squiggly-error').first().waitFor();
      await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      await frame.getByText('-- draft survives stop', {exact: true}).waitFor();
      await sameEditor();
      assert.equal(sessions, 2);
      await other.close();
      // Stop may overtake a session allocation before its ID reaches the page.
      await block.getByRole('button', {name: 'Stop Lean server', exact: true}).click();
      await page.locator('.native-editor:not(.pending)').waitFor();
      let release, allocated;
      const gate = new Promise(resolve => { release = resolve; });
      const allocation = new Promise(resolve => { allocated = resolve; });
      await page.route(/\/lean\/session$/, async route => {
        const response = await route.fetch();
        allocated((await response.json()).id);
        await gate;
        await route.fulfill({response});
      });
      await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      const pendingId = await allocation;
      await block.getByRole('button', {name: 'Stop Lean server', exact: true}).click();
      const retired = page.waitForResponse(r => r.request().method() === 'DELETE' && r.url().endsWith(`/lean/session/${pendingId}`));
      release();
      assert.equal((await retired).status(), 204);
      await frame.getByText('-- draft survives stop', {exact: true}).waitFor();
      assert.equal(await sessionId(page), null);
      assert.equal(sockets, 2, 'a cancelled start must never connect its late session');
      // Navigation while allocating must connect a session prepared for the new node.
      await page.unroute(/\/lean\/session$/);
      let releaseNavigation, allocatedNavigation;
      const navigationGate = new Promise(resolve => { releaseNavigation = resolve; });
      const navigationAllocation = new Promise(resolve => { allocatedNavigation = resolve; });
      await page.route(/\/lean\/session$/, async route => {
        const response = await route.fetch();
        allocatedNavigation((await response.json()).id);
        await navigationGate;
        await route.fulfill({response});
      });
      await block.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      const obsolete = await navigationAllocation;
      await select('First');
      releaseNavigation();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      await frame.getByText('offline0', {exact: true}).waitFor();
      assert.equal((await fetch(`${url}/api/lean/session/${obsolete}`)).status, 404);
      assert.equal(sockets, 3, 'only the newly selected module connects');
    }, nodes);
  } finally { await browser.close(); }
});

await test('collapsed Lean editors recheck without recreating the browser viewport', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const node = {fnode: randomUUID(), title: 'Collapsed reload', module: 'Lib.Collapsed', depens: [], blocks: [
    {srctype: 'lean', content: 'example : True := by trivial\n'},
  ]};
  try {
    await fixture(browser, async ({page, url}) => {
      await page.goto(`${url}/?collapsed-reload#ref=${node.fnode}`);
      await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      const block = page.locator('.lean-block');
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      await block.getByRole('button', {name: 'Collapse block'}).click();
      await block.getByRole('button', {name: 'Recheck Lean'}).click();
      await page.getByText('Lean editor ready', {exact: true}).waitFor();
      assert.equal(await block.getByRole('button', {name: 'Expand block'}).count(), 1, 'recheck preserves the collapsed state');
      assert.equal(await frame.locator('#error').textContent(), '');
      await block.getByRole('button', {name: 'Expand block'}).click();
      await frame.locator('.view-line').first().waitFor();
      const dimensions = await frame.locator('.monaco-editor').evaluate(el => ({width: el.clientWidth, height: el.clientHeight}));
      assert.ok(dimensions.width > 100 && dimensions.height > 100, 'expanded editor has a usable viewport');
    }, [node]);
  } finally { await browser.close(); }
});

await test('editors and previews pass scrolling to the node pane at both boundaries', {timeout: 180000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
  const lines = Array.from({length: 100}, (_, i) => `Line ${i + 1}.`);
  const node = {fnode: randomUUID(), title: 'Scrolling', module: 'Lib.Scrolling', depens: [], blocks: [
    {srctype: 'text', content: lines.join('\n')},
    {srctype: 'latex', content: lines.join('\n\n')},
    {srctype: 'lean', content: lines.map(line => `-- ${line}`).join('\n') + '\nexample : True := by trivial\n'},
    {srctype: 'rocq', content: lines.join('\n')},
  ]};
  const singles = ['latex', 'lean'].map(srctype => ({fnode: randomUUID(), title: `Single ${srctype}`, module: `Lib.Single${srctype}`, depens: [],
    blocks: [{srctype, content: srctype === 'lean' ? 'example : True := by trivial\n' : 'A short LaTeX block.'}]}));
  try {
    await fixture(browser, async ({page, url, cli}) => {
      await page.goto(`${url}/?scroll#ref=${node.fnode}`);
      await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
      const frame = page.frameLocator('iframe[title="Lean source and Infoview"]');
      await frame.locator('.view-line').first().waitFor();
      await page.locator('.native-editor:not(.pending)').waitFor();
      const pane = page.locator('.blocks');
      for (const view of ['Knowledge', 'Graph', 'Knowledge']) {
        await page.getByRole('button', {name: view, exact: true}).click();
        const bounds = await pane.evaluate(el => {
          el.scrollTop = 0;
          const top = el.getBoundingClientRect();
          const next = el.querySelector('[data-srctype="latex"] .block-head').getBoundingClientRect();
          el.scrollTop = el.scrollHeight;
          const last = el.querySelector('[data-srctype="rocq"] .block-head').getBoundingClientRect();
          return {top: top.top, bottom: top.bottom, nextBottom: next.bottom, lastTop: last.top};
        });
        assert.ok(bounds.nextBottom <= bounds.bottom, `${view}: the next header is visible at the top`);
        assert.ok(bounds.lastTop >= bounds.top, `${view}: the last header remains visible above the add button`);
      }
      const outerScroll = () => pane.evaluate(el => el.scrollTop);
      const check = async (block, surface, position) => {
        for (const direction of [1, -1]) {
          await position(direction);
          await block.evaluate(el => {
            const pane = el.closest('.blocks');
            pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top - 20;
          });
          const bounds = await surface.boundingBox();
          await page.mouse.move(bounds.x + 80, bounds.y + 100);
          const before = await outerScroll();
          await page.mouse.wheel(0, direction * 80);
          await page.waitForTimeout(200);
          assert.ok(Math.abs(await outerScroll() - before) < 1, 'scroll stays inside until the editor reaches its boundary');
          // Monaco normalizes Chromium's synthetic wheel ticks to smaller steps.
          for (let step = 0; step < 60 && Math.abs(await outerScroll() - before) < 1; step++) {
            await page.mouse.wheel(0, direction * 500);
            await page.waitForTimeout(100);
          }
          assert.ok((await outerScroll() - before) * direction > 1, `scroll continues in the outer pane at the ${direction > 0 ? 'bottom' : 'top'}`);
        }
      };
      const nativeBoundary = async surface => {
        for (const direction of [1, -1]) {
          const before = await surface.evaluate((el, direction) => {
            const pane = el.closest('.blocks');
            pane.scrollTop += el.closest('article').getBoundingClientRect().top - pane.getBoundingClientRect().top - 20;
            el.scrollTop = direction > 0 ? el.scrollHeight : 0;
            window.boundaryWheel = null;
            window.addEventListener('wheel', event => {
              window.boundaryWheel = {cancelled: event.defaultPrevented, trusted: event.isTrusted};
            }, {once: true});
            return pane.scrollTop;
          }, direction);
          const bounds = await surface.boundingBox();
          await page.mouse.move(bounds.x + 80, bounds.y + 100);
          await page.mouse.wheel(0, direction * 40);
          await page.waitForFunction(({before, direction}) => (document.querySelector('.blocks').scrollTop - before) * direction > 1, {before, direction});
          await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
          const state = await surface.evaluate(el => ({
            wheel: window.boundaryWheel, inner: el.scrollTop, limit: el.scrollHeight - el.clientHeight,
            overflow: getComputedStyle(el).overflowY,
          }));
          assert.deepEqual(state.wheel, {cancelled: false, trusted: true}, 'the original wheel reaches the native outer scroller');
          assert.ok(Math.abs(state.inner - (direction > 0 ? state.limit : 0)) < 1, 'no inner overshoot');
          await page.waitForTimeout(180);
          assert.equal(await surface.evaluate(el => getComputedStyle(el).overflowY), 'auto', 'restore inner scrolling after the gesture');
        }
      };
      const latex = page.locator('[data-srctype="latex"]');
      const code = latex.locator('.editor-scroll');
      await check(latex, code, direction => code.evaluate((el, direction) => { el.scrollTop = direction > 0 ? 0 : el.scrollHeight; }, direction));
      await nativeBoundary(code);
      await latex.getByRole('button', {name: 'Render LaTeX preview'}).click();
      const preview = latex.locator('.latex-preview');
      await preview.waitFor();
      await check(latex, preview, direction => preview.evaluate((el, direction) => { el.scrollTop = direction > 0 ? 0 : el.scrollHeight; }, direction));
      await nativeBoundary(preview);
      const input = frame.getByRole('textbox', {name: /Editor content/});
      await check(page.locator('[data-srctype="lean"]'), frame.locator('.monaco-editor'), async direction => {
        await input.press(direction > 0 ? 'ControlOrMeta+Home' : 'ControlOrMeta+End');
      });
      for (const reducedMotion of ['no-preference', 'reduce']) {
        await page.emulateMedia({reducedMotion});
        for (const single of singles) {
          const kind = single.blocks[0].srctype;
          await page.goto(`${url}/?scroll=${kind}#ref=${single.fnode}`);
          await title(page, single.title);
          const surfaces = [];
          if (kind === 'latex') surfaces.push(page.locator('.editor-scroll'));
          else {
            await page.getByRole('button', {name: 'Start Lean server', exact: true}).click();
            await page.locator('.native-editor:not(.pending)').waitFor();
            surfaces.push(frame.locator('.monaco-editor'));
            await frame.frameLocator('#infoview iframe').getByText('All Messages', {exact: true}).waitFor();
            surfaces.push(frame.frameLocator('#infoview iframe').locator('html'));
          }
          for (const surface of surfaces) {
            for (const direction of [1, -1]) {
              await pane.evaluate((el, direction) => { el.scrollTop = direction > 0 ? el.scrollHeight : 0; }, direction);
              await surface.evaluate(el => {
                const win = el.ownerDocument.defaultView;
                win.boundaryWheel = null;
                win.addEventListener('wheel', event => {
                  win.boundaryWheel = {cancelled: event.defaultPrevented, trusted: event.isTrusted};
                }, {once: true});
              });
              const bounds = await surface.boundingBox();
              await page.mouse.move(bounds.x + 80, bounds.y + 80);
              await page.mouse.wheel(0, direction * 40);
              await surface.evaluate(el => new Promise(resolve => el.ownerDocument.defaultView.requestAnimationFrame(() => requestAnimationFrame(resolve))));
              const wheel = await surface.evaluate(el => el.ownerDocument.defaultView.boundaryWheel);
              assert.deepEqual(wheel, {cancelled: false, trusted: true}, `${kind} preserves native boundary handling with ${reducedMotion}`);
              assert.equal(await pane.evaluate(el => getComputedStyle(el).overscrollBehaviorY), 'contain', 'the outer pane terminates the chain and keeps native bounce');
            }
          }
        }
      }
      if (process.env.MDC_E2E_NATIVE_SCROLL === '1') {
        assert.equal(process.platform, 'darwin', 'native scroll checks require macOS');
        const root = await mkdtemp(resolve(tmpdir(), 'mdc-native-scroll-'));
        try {
          const probe = resolve(root, 'probe');
          await run('swiftc', [resolve(webRoot, 'e2e/native-scroll.swift'), '-module-cache-path', resolve(root, 'modules'), '-o', probe]);
          const empty = JSON.parse((await cli('show', 'Alpha')).stdout).fnode;
          const result = await run(probe, [url, node.fnode, ...singles.map(n => n.fnode), empty], {timeout: 90000});
          console.log(result.stdout);
        } finally { await rm(root, {recursive: true, force: true}); }
      }
    }, [node, ...singles]);
  } finally { await browser.close(); }
});

await test('view changes reveal measured editors and loaded pages without intermediate frames', {timeout: 60000}, async () => {
  const browser = process.env.MDC_E2E_BROWSER === 'webkit' ? await webkit.launch() : await chromium.launch({headless: true, channel: 'chromium'});
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
      await page.evaluate(() => { window.originalEditor = document.querySelector('.monaco-editor'); });
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
              const editor = document.querySelector('.monaco-editor');
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
      assert.equal(await page.evaluate(() => window.originalEditor === document.querySelector('.monaco-editor')), true);
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
