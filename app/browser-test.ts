import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { Store } from "../coordinator/src/store.js";
import { server } from "../coordinator/src/http.js";
import { LeanGround } from "../coordinator/src/remote.js";
import { Documents } from "../coordinator/src/documents.js";
import { board, fact } from "../coordinator/test/fixtures.js";
import { ingest } from "../coordinator/src/domain.js";
import type { Config } from "../coordinator/src/config.js";
const dsn = process.env.MDC_TEST_DATABASE_URL;
if (!dsn) throw new Error("MDC_TEST_DATABASE_URL required");
const schema = "browser_" + randomUUID().replaceAll("-", "");
const admin = new Store(dsn);
await admin.pool.query(`CREATE SCHEMA ${schema}`);
const url = new URL(dsn);
url.searchParams.set("options", `-c search_path=${schema}`);
const store = new Store(url.toString());
await store.migrate();
const b = board();
b.title = "共享引理 · 协作证明";
b.goals.T.statement = "∀ n : Nat, n = n";
ingest(b, fact("t", "T", ["A", "B"]));
ingest(b, fact("a", "A"));
b.last_sync = Date.now();
await store.create(b);
const cfg: Config = {
  dsn: dsn,
  host: "127.0.0.1",
  port: 0,
  actors: { alice: { token: "browser-test-token", admin: true } },
  leanUrl: "http://127.0.0.1:1",
  leanToken: "unused",
  leanActor: "coordinator",
  terminusUser: "admin",
  appDir: resolve("dist"),
};
const http = server(
  cfg,
  store,
  new LeanGround(cfg.leanUrl, cfg.leanToken),
  new Documents(),
);
http.listen(0, "127.0.0.1");
await once(http, "listening");
const browser = await chromium.launch({
  headless: true,
  ...(process.env.MDC_CHROME_PATH
    ? { executablePath: process.env.MDC_CHROME_PATH }
    : {}),
});
const page = await browser.newPage({ viewport: { width: 1360, height: 950 } });
const errors: string[] = [];
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto(
    `http://127.0.0.1:${(http.address() as { port: number }).port}`,
  );
  await page.getByLabel("访问令牌").fill("browser-test-token");
  await page.getByRole("button", { name: "进入工作台 →" }).click();
  await page.getByRole("button", { name: /共享引理/ }).click();
  await page
    .getByRole("heading", { name: "共享引理 · 协作证明", exact: true })
    .waitFor();
  const artifacts = process.env.MDC_BROWSER_ARTIFACTS ?? "/tmp/mathdoc-browser";
  await mkdir(artifacts, { recursive: true });
  await page.screenshot({ path: `${artifacts}/overview.png`, fullPage: true });
  await page.getByRole("button", { name: "任务", exact: true }).click();
  await page
    .locator("button:enabled")
    .filter({ hasText: /^领取任务$/ })
    .first()
    .click();
  await page.getByRole("button", { name: "续租", exact: true }).waitFor();
  assert.equal(
    Object.values((await store.get("project", "alice")).attempts).length,
    1,
  );
  await page.getByRole("button", { name: "续租", exact: true }).click();
  await page.getByRole("button", { name: "结束尝试", exact: true }).click();
  await page.waitForFunction(
    () =>
      !Array.from(document.querySelectorAll("button")).some(
        (b) => b.textContent === "续租",
      ),
  );
  assert.equal((await store.get("project", "alice")).spent, 10);
  await page.getByRole("button", { name: "拆法", exact: true }).click();
  await page.getByRole("button", { name: "暂停", exact: true }).click();
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll("button")).some(
      (b) => b.textContent === "暂停" && b.disabled,
    ),
  );
  assert.equal((await store.get("project", "alice")).facts.t.state, "paused");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: `${artifacts}/mobile.png`, fullPage: true });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    ),
    false,
  );
  assert.deepEqual(errors, []);
  console.log(
    "Browser passed: login, project, claim/renew/finish, route pause, mobile layout; " +
      artifacts,
  );
} finally {
  await browser.close();
  http.closeAllConnections();
  await new Promise<void>((r) => http.close(() => r()));
  await store.pool.end();
  await admin.pool.query(`DROP SCHEMA ${schema} CASCADE`);
  await admin.pool.end();
}
