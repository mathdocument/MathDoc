import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { databaseUrl, secret } from "../src/config.js";

test("secrets come from a variable or a file, never both, never empty", async () => {
  const dir = await mkdtemp(join(tmpdir(), "mdc-secret-"));
  const file = join(dir, "password");
  await writeFile(file, "  s3cret\n");
  assert.equal(secret("X", { X: "direct" }), "direct");
  assert.equal(secret("X", { X_FILE: file }), "s3cret");
  assert.equal(secret("X", {}), undefined);
  assert.throws(
    () => secret("X", { X: "a", X_FILE: file }),
    /set only one of X and X_FILE/,
  );
  assert.throws(
    () => secret("X", { X_FILE: join(dir, "missing") }),
    /cannot read X_FILE/,
  );
  const empty = join(dir, "empty");
  await writeFile(empty, "\n");
  assert.throws(() => secret("X", { X_FILE: empty }), /is empty/);
  // The database password joins the URL, escaped.
  await writeFile(file, "p@ss/word\n");
  assert.equal(
    databaseUrl({
      MDC_DATABASE_URL: "postgresql://mathdoc@postgres:5432/mathdoc",
      MDC_DATABASE_PASSWORD_FILE: file,
    }),
    "postgresql://mathdoc:p%40ss%2Fword@postgres:5432/mathdoc",
  );
  assert.throws(() => databaseUrl({}), /MDC_DATABASE_URL required/);
});
