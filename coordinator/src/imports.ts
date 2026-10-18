// External proof imports (docs/external-proofs.md §3.3). One Comparator configuration in a
// pinned upstream repository becomes one MathDoc database: text nodes for the papers, the scope
// note and the proof's summary; the statement and every file of the proof's import closure as
// nodes holding the upstream text in a Lean block, linked by their imports (the blocks carry
// where the text came from, and show as imported while unchanged); and a record of its project
// checks. The checks
// run in LeanGround (POST /v1/project-checks); their records live here, not on a board, since
// they neither combine by GoalKey nor take part in routes.
//
// Upstream files are read with git at the import's commit, never from the working tree, so a
// later checkout does not change what an import shows, and a requested path cannot leave the
// repository. Source browsing serves only the statement module and the proof's import closure.
import { spawn, execFile } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { posix } from "node:path";
import { promisify } from "node:util";
import type pg from "pg";
import { z } from "zod";
import { importedMetadata, type ImportedSource } from "./certification.js";
import { Fault, requireThat } from "./domain.js";
import type { LeanGround } from "./remote.js";

const run = promisify(execFile);

export const IMPORT_MIGRATION = `
CREATE TABLE IF NOT EXISTS mdc_import(database text PRIMARY KEY, body jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS mdc_import_check(id text PRIMARY KEY, database text NOT NULL REFERENCES mdc_import(database) ON DELETE CASCADE, body jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX IF NOT EXISTS mdc_import_check_database ON mdc_import_check(database, created_at);
`;

const sourceSchema = z
  .object({
    /** The git repository (its root). */
    repo: z.string().min(1),
    /** The Lake project inside it, relative to the root. */
    lean: z.string().default("lean"),
    /** The LeanGround project-check workspace holding the same checkout. */
    workspace: z.string().min(1),
    /** Where the repository is published, for links (GitHub style: /blob/<commit>/<path>). */
    url: z.string().url().optional(),
  })
  .strict();
export type ImportSource = z.infer<typeof sourceSchema>;

/** MDC_IMPORT_SOURCES: a JSON object from source names to sources. */
export function importSources(raw?: string): Record<string, ImportSource> {
  if (!raw) return {};
  return z.record(z.string().regex(/^[A-Za-z0-9_-]+$/), sourceSchema).parse(JSON.parse(raw));
}

const comparatorSchema = z
  .object({
    challenge_module: z.string(),
    solution_module: z.string(),
    theorem_names: z.array(z.string()),
    definition_names: z.array(z.string()).default([]),
    permitted_axioms: z.array(z.string()),
    enable_nanoda: z.boolean().optional(),
  })
  .passthrough();
export type ComparatorConfig = z.infer<typeof comparatorSchema>;

const MODULE = /^[A-Za-z_][A-Za-z0-9_']*(\.[A-Za-z_][A-Za-z0-9_']*)*$/;
const CONFIG = /^[A-Za-z0-9_-]{1,100}$/;

/** Reads one repository at one commit. */
export class Upstream {
  constructor(
    readonly repo: string,
    readonly commit: string,
  ) {}

  static async head(repo: string): Promise<string> {
    const { stdout } = await run("git", ["-C", repo, "rev-parse", "HEAD"]);
    return stdout.trim();
  }

  /** Each path's text at the commit, or null where there is no such file. One git process. */
  async read(paths: string[]): Promise<Map<string, string | null>> {
    const out = new Map<string, string | null>();
    // `<commit>:../x` makes git cat-file exit instead of answering `missing`, and a newline would
    // break the batch protocol: such paths are not files of the repository.
    const safe = (p: string) =>
      p !== "" && !p.startsWith("/") && !/[\n\r\0]/.test(p) &&
      p.split("/").every((seg) => seg !== "" && seg !== "." && seg !== "..");
    for (const p of paths) if (!safe(p)) out.set(p, null);
    paths = paths.filter(safe);
    if (!paths.length) return out;
    const proc = spawn("git", ["-C", this.repo, "cat-file", "--batch"]);
    const chunks: Buffer[] = [];
    proc.stdout.on("data", (c: Buffer) => chunks.push(c));
    const done = new Promise<void>((resolve, reject) => {
      proc.on("error", reject);
      proc.on("close", (code) =>
        code === 0 ? resolve() : reject(new Error(`git cat-file exited with ${code}`)),
      );
    });
    proc.stdin.end(paths.map((p) => `${this.commit}:${p}\n`).join(""));
    await done;
    const buf = Buffer.concat(chunks);
    let at = 0;
    for (const p of paths) {
      const nl = buf.indexOf(10, at);
      const header = buf.subarray(at, nl).toString();
      at = nl + 1;
      if (header.endsWith(" missing") || header.endsWith(" ambiguous")) {
        out.set(p, null);
        continue;
      }
      const [, type, size] = header.split(" ");
      const n = Number(size);
      out.set(p, type === "blob" ? buf.subarray(at, at + n).toString("utf8") : null);
      at += n + 1;
    }
    return out;
  }

  async text(path: string): Promise<string | null> {
    return (await this.read([path])).get(path) ?? null;
  }

  /** Files under `dir` (recursively) whose text contains `fixed`. */
  async grep(fixed: string, dir: string): Promise<string[]> {
    try {
      const { stdout } = await run(
        "git",
        ["-C", this.repo, "grep", "-l", "-F", fixed, this.commit, "--", dir],
        { maxBuffer: 16 << 20 },
      );
      return stdout
        .split("\n")
        .filter(Boolean)
        .map((l) => l.slice(this.commit.length + 1))
        .sort();
    } catch (e) {
      if ((e as { code?: number }).code === 1) return []; // no match
      throw e;
    }
  }
}

/** The modules a Lean file imports: the header's `import` lines, before any other command. */
export function leanImports(text: string): string[] {
  const out: string[] = [];
  let comment = 0;
  for (const raw of text.split("\n")) {
    let line = raw.trim();
    if (comment) {
      const close = line.indexOf("-/");
      if (close < 0) continue;
      comment = 0;
      line = line.slice(close + 2).trim();
    }
    if (line.startsWith("/-")) {
      if (!line.includes("-/")) comment = 1;
      continue;
    }
    if (!line || line.startsWith("--") || line === "module" || line === "prelude") continue;
    const m = /^(?:(?:public|meta|private)\s+)*import\s+(?:all\s+)?(.+)$/.exec(line);
    if (!m) break;
    out.push(...m[1].split(/\s+/).filter(Boolean));
  }
  return out;
}

export interface SourceModule {
  module: string;
  path: string;
  lines: number;
  /** Imports inside the repository. */
  imports: string[];
}
export interface Closure {
  modules: SourceModule[];
  /** Roots of imported modules outside the repository, e.g. Mathlib. */
  external: string[];
}

const modulePath = (lean: string, module: string) =>
  posix.join(lean, ...module.split(".")) + ".lean";

/** The import closure of `root` inside the repository's Lake project, read in rounds of one git call. */
export async function closure(up: Upstream, lean: string, root: string): Promise<Closure> {
  const seen = new Map<string, SourceModule>();
  const external = new Set<string>();
  let frontier = [root];
  while (frontier.length) {
    const texts = await up.read(frontier.map((m) => modulePath(lean, m)));
    const next = new Set<string>();
    for (const m of frontier) {
      const text = texts.get(modulePath(lean, m));
      if (text == null) {
        external.add(m.split(".")[0]);
        continue;
      }
      const imports = leanImports(text);
      seen.set(m, { module: m, path: modulePath(lean, m), lines: text.split("\n").length - 1, imports });
      for (const i of imports) if (!seen.has(i)) next.add(i);
    }
    frontier = [...next].filter((m) => !seen.has(m));
  }
  // `imports` lists only modules found in the repository.
  for (const s of seen.values()) s.imports = s.imports.filter((i) => seen.has(i));
  return { modules: [...seen.values()].sort((a, b) => a.module.localeCompare(b.module)), external: [...external].sort() };
}

export interface ImportRecord {
  database: string;
  source: string;
  url?: string;
  commit: string;
  config_name: string;
  config: ComparatorConfig;
  /** The scope note (lean/docs/NNN.md) that links this configuration. */
  scope: { path: string; family: string; title: string; result: string | null };
  papers: { title: string; path: string; present: boolean }[];
  statement: { module: string; path: string; lines: number };
  proof: { module: string; path: string; files: number; lines: number; external: string[] };
  /** `modules`: the node of each file of the proof's closure (absent on imports built before). */
  nodes: {
    overview: string;
    papers: string[];
    scope: string;
    statement: string;
    proof: string;
    modules?: Record<string, string>;
  };
  created_by: string;
  created_at: string;
  rebuilt_by?: string;
  rebuilt_at?: string;
}

/** A node UUID derived from the database and the node's role, so a rebuilt import keeps its ids. */
function nodeId(database: string, role: string) {
  const h = createHash("sha256").update(`mdc-import:${database}:${role}`).digest("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-5${h.slice(13, 16)}-a${h.slice(17, 20)}-${h.slice(20, 32)}`;
}

/** Markdown links relative to `from` rewritten to the published repository at the commit. */
function absoluteLinks(markdown: string, from: string, link: (path: string) => string) {
  return markdown.replace(/\]\(([^)\s]+)\)/g, (whole, target: string) => {
    if (/^[a-z]+:|^#|^\//i.test(target)) return whole;
    const [path, hash] = target.split("#");
    const resolved = posix.normalize(posix.join(posix.dirname(from), path));
    if (resolved.startsWith("..")) return whole;
    return `](${link(resolved)}${hash ? `#${hash}` : ""})`;
  });
}

/** What importing configuration `name` from `source` would create: the record and the graph. */
export async function buildImport(
  database: string,
  sourceName: string,
  source: ImportSource,
  name: string,
  actor: string,
  commit?: string,
) {
  requireThat(CONFIG.test(name), "invalid_import_config", 400);
  const up = new Upstream(source.repo, commit ?? (await Upstream.head(source.repo)));
  const lean = source.lean;
  const configPath = posix.join(lean, "ComparatorChallenges", `${name}.json`);
  const configText = await up.text(configPath);
  requireThat(configText !== null, "import_config_not_found", 404);
  const config = comparatorSchema.parse(JSON.parse(configText!));
  for (const m of [config.challenge_module, config.solution_module])
    requireThat(MODULE.test(m), "invalid_import_config", 400);
  const statementPath = modulePath(lean, config.challenge_module);
  const statementText = await up.text(statementPath);
  requireThat(statementText !== null, "import_statement_not_found", 404);
  const proof = await closure(up, lean, config.solution_module);
  requireThat(
    proof.modules.some((m) => m.module === config.solution_module),
    "import_proof_not_found",
    404,
  );

  // The scope note is the one whose Comparator links name this configuration's statement.
  const notes = await up.grep(`ComparatorChallenges/${name}.lean`, posix.join(lean, "docs"));
  requireThat(notes.length > 0, "import_scope_not_found", 404);
  const scopePath = notes[0];
  const scopeText = (await up.text(scopePath))!;
  const title = /^#\s+(.+)$/m.exec(scopeText)?.[1].trim() ?? name;
  const row = scopeText
    .split("\n")
    .find((l) => l.startsWith("|") && l.includes(`ComparatorChallenges/${name}.lean`));
  const result = row ? (row.split("|")[1]?.trim() ?? null) : null;
  const paperLinks = [...scopeText.matchAll(/^- \[([^\]]+)\]\(([^)]+\.pdf)\)/gm)].map((m) => ({
    title: m[1],
    path: posix.normalize(posix.join(posix.dirname(scopePath), m[2])),
  }));
  const present = await up.read(paperLinks.map((p) => p.path).filter((p) => !p.startsWith("..")));
  const papers = paperLinks.map((p) => ({ ...p, present: Boolean(present.get(p.path)) }));

  const link = (path: string) =>
    source.url ? `${source.url.replace(/\/$/, "")}/blob/${up.commit}/${path}` : path;
  const files = proof.modules.length;
  const lines = proof.modules.reduce((s, m) => s + m.lines, 0);
  const ids = {
    overview: nodeId(database, "overview"),
    papers: papers.map((_, i) => nodeId(database, `paper-${i}`)),
    scope: nodeId(database, "scope"),
    statement: nodeId(database, "statement"),
    proof: nodeId(database, "proof"),
    modules: Object.fromEntries(
      proof.modules.map((m) => [m.module, nodeId(database, `module:${m.module}`)]),
    ),
  };
  const texts = await up.read(proof.modules.map((m) => m.path));
  const family = posix.basename(scopePath, ".md");
  const code = (s: string) => `\`${s}\``;
  const list = (xs: string[]) => (xs.length ? xs.map(code).join(", ") : "none");
  const overview = [
    `# ${result ?? title}`,
    "",
    `${title}. Imported from ${source.url ? `[${source.url}](${source.url}/tree/${up.commit})` : code(sourceName)} at commit ${code(up.commit)}, Comparator configuration ${source.url ? `[${code(name)}](${link(configPath)})` : code(name)}.`,
    "",
    `- **Statement**: ${code(config.challenge_module)}`,
    `- **Proof**: ${code(config.solution_module)}, ${files} files and ${lines} lines in the repository; also imports ${list(proof.external)}`,
    `- **Theorems**: ${list(config.theorem_names)}`,
    `- **Definition holes**: ${list(config.definition_names)}`,
    `- **Permitted axioms**: ${list(config.permitted_axioms)}`,
    "",
    "The import page shows the project checks and the proof's source.",
  ].join("\n");
  const node = (
    fnode: string,
    title: string,
    content: string,
    depens: string[],
    lean?: { source: ImportedSource; text: string },
  ) => ({
    fnode,
    title,
    module: `Imported.N_${fnode.replaceAll("-", "")}`,
    depens,
    blocks: [
      { srctype: "text", content, metadata: {} },
      ...(lean
        ? [{ srctype: "lean", content: lean.text, metadata: importedMetadata(lean.source, lean.text) }]
        : []),
    ],
  });
  const fileNode = (m: SourceModule) => {
    const text = texts.get(m.path)!;
    const outside = leanImports(text).filter((i) => !(i in ids.modules));
    return node(
      ids.modules[m.module],
      m.module,
      [
        `# ${code(m.module)}`,
        "",
        `${m.module === config.solution_module ? "The proof's main file: " : ""}[${m.path}](${link(m.path)}), ${m.lines} lines at commit ${code(up.commit.slice(0, 12))}. The Lean block is the upstream text, unchanged; the project check covers it as part of the proof.`,
        ...(outside.length ? ["", `Imports outside the repository: ${list(outside)}.`] : []),
      ].join("\n"),
      m.imports.map((i) => ids.modules[i]),
      { source: { module: m.module, path: m.path, commit: up.commit }, text },
    );
  };
  const toolchain = (await up.text(posix.join(lean, "lean-toolchain")))?.trim();
  const graph = {
    nodes: [
      node(ids.overview, `${result ?? title}`, overview, [ids.statement, ids.proof]),
      ...papers.map((p, i) =>
        node(ids.papers[i], `Paper · ${p.title}`,
          `# ${p.title}\n\n[${p.path}](${link(p.path)})${p.present ? "" : "\n\nThe PDF is not in the repository at this commit."}`,
          []),
      ),
      node(ids.scope, `Scope note · ${family}`, absoluteLinks(scopeText, scopePath, link), ids.papers),
      node(ids.statement, `Statement · ${config.challenge_module}`,
        `# Statement\n\n${code(config.challenge_module)} ([source](${link(statementPath)})). The theorems and definitions in the Lean block are what a passing check guarantees; definition holes are filled by the proof.`,
        [ids.scope],
        { source: { module: config.challenge_module, path: statementPath, commit: up.commit }, text: statementText! }),
      node(ids.proof, `Proof · ${config.solution_module}`,
        `# Proof\n\n${code(config.solution_module)} ([source](${link(modulePath(lean, config.solution_module))})) imports ${files - 1} other modules of the repository (${files} files, ${lines} lines in all) and ${list(proof.external)}. Each file is a node below this one, its upstream text in a Lean block; a file depends on the files it imports.`,
        [ids.statement, ids.modules[config.solution_module]]),
      ...proof.modules.map(fileNode),
    ],
    project: {
      toolchain: toolchain?.startsWith("leanprover/lean4:v") ? toolchain : "leanprover/lean4:v4.34.1",
      lakefile: 'name = "Imported"\nversion = "0.1.0"\n\n[[lean_lib]]\nname = "Imported"\n',
      manifest: null,
      module_root: "Imported",
    },
  };
  const record: ImportRecord = {
    database,
    source: sourceName,
    ...(source.url ? { url: source.url } : {}),
    commit: up.commit,
    config_name: name,
    config,
    scope: { path: scopePath, family, title, result },
    papers,
    statement: { module: config.challenge_module, path: statementPath, lines: statementText!.split("\n").length - 1 },
    proof: { module: config.solution_module, path: modulePath(lean, config.solution_module), files, lines, external: proof.external },
    nodes: ids,
    created_by: actor,
    created_at: new Date().toISOString(),
  };
  return { record, graph };
}

export interface ImportCheck {
  id: string;
  /** Sent to LeanGround; resending after a lost reply returns the same check. */
  idempotency_key: string;
  status: "submitting" | "queued" | "running" | "passed" | "failed" | "error" | "submit_failed";
  leanground_check?: string;
  submitted_by: string;
  submitted_at: string;
  completed_at?: string;
  error?: string;
  result?: unknown;
}

type Db = pg.Pool | pg.PoolClient;

/** Import records and their checks in PostgreSQL. */
export class Imports {
  private polling = new Map<string, Promise<void>>();
  private closures = new Map<string, Promise<Closure>>();

  constructor(
    private pool: pg.Pool,
    private lean: LeanGround,
    readonly sources: Record<string, ImportSource>,
  ) {}

  async get(database: string, db: Db = this.pool): Promise<ImportRecord | null> {
    const r = await db.query("SELECT body FROM mdc_import WHERE database=$1", [database]);
    return (r.rows[0]?.body as ImportRecord) ?? null;
  }
  async list(): Promise<ImportRecord[]> {
    const r = await this.pool.query("SELECT body FROM mdc_import ORDER BY database");
    return r.rows.map((x) => x.body as ImportRecord);
  }
  async put(record: ImportRecord) {
    await this.pool.query(
      "INSERT INTO mdc_import(database, body) VALUES ($1,$2)",
      [record.database, record],
    );
  }
  async update(record: ImportRecord) {
    await this.pool.query("UPDATE mdc_import SET body=$2 WHERE database=$1", [record.database, record]);
  }
  async drop(database: string) {
    await this.pool.query("DELETE FROM mdc_import WHERE database=$1", [database]);
  }
  async checks(database: string): Promise<ImportCheck[]> {
    await this.refresh(database);
    const r = await this.pool.query(
      "SELECT body FROM mdc_import_check WHERE database=$1 ORDER BY created_at DESC, id",
      [database],
    );
    return r.rows.map((x) => x.body as ImportCheck);
  }
  private async save(database: string, c: ImportCheck) {
    await this.pool.query(
      `INSERT INTO mdc_import_check(id, database, body) VALUES ($1,$2,$3)
       ON CONFLICT (id) DO UPDATE SET body = EXCLUDED.body`,
      [c.id, database, c],
    );
  }

  /** Submit the import's configuration to LeanGround. A check still in progress is returned instead. */
  async submit(database: string, actor: string): Promise<ImportCheck> {
    const record = await this.get(database);
    requireThat(record, "not_found", 404);
    const source = this.sources[record!.source];
    requireThat(source, "import_source_not_configured", 503);
    const checks = await this.checks(database);
    const running = checks.find((c) => c.status === "queued" || c.status === "running");
    if (running) return running;
    // A check whose submission failed or whose reply was lost is resent with the same key:
    // LeanGround runs it once.
    const check: ImportCheck = checks.find(
      (c) => c.status === "submitting" || c.status === "submit_failed",
    ) ?? {
      id: randomUUID(),
      idempotency_key: `mdc-import-${database}-${randomUUID()}`,
      status: "submitting",
      submitted_by: actor,
      submitted_at: new Date().toISOString(),
    };
    check.status = "submitting";
    await this.save(database, check);
    try {
      const reply = await this.lean.projectCheck({
        idempotency_key: check.idempotency_key,
        workspace: source!.workspace,
        config: {
          challenge_module: record!.config.challenge_module,
          solution_module: record!.config.solution_module,
          theorem_names: record!.config.theorem_names,
          definition_names: record!.config.definition_names,
          permitted_axioms: record!.config.permitted_axioms,
        },
      });
      Object.assign(check, this.from(reply));
      delete check.error;
    } catch (e) {
      check.status = "submit_failed";
      check.error = e instanceof Fault ? e.code : "leanground_unavailable";
    }
    await this.save(database, check);
    return check;
  }

  private from(reply: unknown): Partial<ImportCheck> {
    const r = z
      .object({
        check_id: z.string(),
        status: z.enum(["queued", "running", "passed", "failed", "error"]),
        completed_at: z.string().nullable().optional(),
        result: z.unknown().optional(),
      })
      .passthrough()
      .parse(reply);
    return {
      leanground_check: r.check_id,
      status: r.status,
      ...(r.completed_at ? { completed_at: r.completed_at } : {}),
      ...(r.result != null ? { result: r.result } : {}),
    };
  }

  /** Ask LeanGround about checks still in progress; one poll per database at a time. */
  async refresh(database: string) {
    const running = this.polling.get(database);
    if (running) return running;
    const poll = (async () => {
      const r = await this.pool.query(
        "SELECT body FROM mdc_import_check WHERE database=$1 AND body->>'status' = ANY($2)",
        [database, ["queued", "running"]],
      );
      for (const row of r.rows) {
        const c = row.body as ImportCheck;
        if (!c.leanground_check) continue;
        try {
          const reply = await this.lean.getProjectCheck(c.leanground_check);
          const next = { ...c, ...this.from(reply) };
          if (next.status !== c.status) await this.save(database, next);
        } catch {
          // LeanGround unreachable: the check stays as it was and is asked again next time.
        }
      }
    })().finally(() => this.polling.delete(database));
    this.polling.set(database, poll);
    return poll;
  }

  /** The statement module and the proof's import closure, at the import's commit. */
  async source(record: ImportRecord) {
    const source = this.sources[record.source];
    requireThat(source, "import_source_not_configured", 503);
    const key = `${source!.repo}\0${record.commit}\0${record.proof.module}`;
    let c = this.closures.get(key);
    if (!c) {
      c = closure(new Upstream(source!.repo, record.commit), source!.lean, record.proof.module);
      c.catch(() => this.closures.delete(key));
      this.closures.set(key, c);
    }
    const proof = await c;
    return {
      commit: record.commit,
      url: record.url ?? null,
      lean: source!.lean,
      statement: record.statement,
      root: record.proof.module,
      modules: proof.modules,
      external: proof.external,
    };
  }

  /** One module's text: the statement module or a module of the proof's closure. */
  async file(record: ImportRecord, module: string) {
    requireThat(MODULE.test(module), "not_found", 404);
    const tree = await this.source(record);
    const entry =
      module === record.statement.module
        ? { module, path: record.statement.path }
        : tree.modules.find((m) => m.module === module);
    requireThat(entry, "not_found", 404);
    const source = this.sources[record.source]!;
    const text = await new Upstream(source.repo, record.commit).text(entry!.path);
    requireThat(text !== null, "not_found", 404);
    return {
      module,
      path: entry!.path,
      commit: record.commit,
      url: record.url ? `${record.url.replace(/\/$/, "")}/blob/${record.commit}/${entry!.path}` : null,
      text: text!,
    };
  }
}
