import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { Documents } from "../src/documents.js";
test("document adapter preserves unrelated blocks, checks dependency cycles, removes stale certification metadata on edits", async () => {
  const first = Documents.empty("A"),
    second = Documents.empty("B");
  first.blocks = [
    {
      srctype: "lean",
      content: "old",
      metadata: {
        certification_id: "old-cert",
        coordination_operation: "old-op",
      },
    },
    { srctype: "rocq", content: "unchanged", metadata: { custom: "keep" } },
  ];
  second.depens = [first.fnode];
  let version = "v1";
  let received: any;
  const http = createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = JSON.parse(Buffer.concat(chunks).toString() || "{}");
    res.setHeader("TerminusDB-Data-Version", version);
    res.setHeader("content-type", "application/json");
    if (req.method === "GET") {
      res.end(
        JSON.stringify(
          [first, second].map((n) => ({
            "@type": "Node",
            ...n,
            depens: n.depens.map((id) => "Node/" + id),
            blocks: JSON.stringify(n.blocks),
          })),
        ),
      );
    } else {
      assert.equal(req.headers["terminusdb-data-version"], version);
      received = body[0];
      version = "v2";
      res.setHeader("TerminusDB-Data-Version", version);
      res.end("[]");
    }
  });
  http.listen(0, "127.0.0.1");
  await once(http, "listening");
  const adapter = new Documents(
    `http://127.0.0.1:${(http.address() as { port: number }).port}`,
    "admin",
    "password",
  );
  try {
    const ref = { database: "docs", branch: "main" };
    const cyclic = structuredClone(first);
    cyclic.depens = [second.fnode];
    await assert.rejects(
      () => adapter.put(ref, cyclic, "v1", "alice"),
      /dependency_cycle/,
    );
    const changed = structuredClone(first);
    changed.blocks[0].content = "new";
    await adapter.put(ref, changed, "v1", "alice");
    const blocks = JSON.parse(received.blocks);
    assert.equal(blocks[0].metadata.certification_id, undefined);
    assert.equal(blocks[1].content, "unchanged");
    assert.equal(blocks[1].metadata.custom, "keep");
    await assert.rejects(
      () => adapter.put(ref, changed, "v1", "alice"),
      /document_revision_conflict/,
    );
  } finally {
    http.closeAllConnections();
    await new Promise<void>((r) => http.close(() => r()));
  }
});
