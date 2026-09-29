import { config } from "./config.js";
import { Store } from "./store.js";
import { LeanGround } from "./remote.js";
import { Documents } from "./documents.js";
import { Worker } from "./worker.js";
import { Terminus } from "./docs/terminus.js";
import { server } from "./http.js";
import { setTimeout as delay } from "node:timers/promises";
const mode = process.argv[2] ?? "serve";
if (!["serve", "worker", "migrate"].includes(mode))
  throw new Error("usage: node coordinator/dist/main.js serve|worker|migrate");
// Migration only needs PostgreSQL; it must not require service credentials.
if (mode === "migrate") {
  if (!process.env.MDC_DATABASE_URL)
    throw new Error("MDC_DATABASE_URL required");
  const store = new Store(process.env.MDC_DATABASE_URL);
  try {
    await store.migrate();
    console.log("MathDoc coordination schema ready");
  } finally {
    await store.pool.end();
  }
} else {
  const cfg = config();
  const store = new Store(cfg.dsn);
  const lean = new LeanGround(cfg.leanUrl, cfg.leanToken);
  const docs = new Documents(
    cfg.terminusUrl,
    cfg.terminusUser,
    cfg.terminusPassword,
  );
  if (mode === "serve") {
    const http = server(cfg, store, lean, docs);
    http.requestTimeout = 150000;
    http.headersTimeout = 10000;
    http.listen(cfg.port, cfg.host, () =>
      console.log(`MathDoc http://${cfg.host}:${cfg.port}`),
    );
    const close = () => http.close(() => void store.pool.end());
    process.once("SIGTERM", close);
    process.once("SIGINT", close);
  } else {
    let stopped = false;
    process.once("SIGTERM", () => {
      stopped = true;
    });
    process.once("SIGINT", () => {
      stopped = true;
    });
    const terminus =
      cfg.terminusUrl && cfg.terminusPassword
        ? new Terminus({
            url: cfg.terminusUrl,
            user: cfg.terminusUser,
            password: cfg.terminusPassword,
          })
        : null;
    const worker = new Worker(store, lean, docs, cfg.leanActor, terminus);
    while (!stopped) {
      try {
        if (!(await worker.tick())) await delay(1000);
      } catch (e) {
        console.error(
          "worker cycle failed",
          e instanceof Error ? e.name : "unknown",
        );
        await delay(1000);
      }
    }
    await store.pool.end();
  }
}
