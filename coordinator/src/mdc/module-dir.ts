// Goal directories of module projects (docs/mathdoc.md §4.13). `mdc pull` writes the
// goal's statement files as the starting contribution, the shared modules they import (read
// only) and GOAL.md; `mdc check` / `prove` / `split` send every other `.lean` file of the
// directory as the contribution. Files are sent as they are: LeanGround compiles them and its
// kernel decides, so mdc only checks what it can know without Lean (paths, the shared files).
import {
  chmod,
  mkdir,
  readdir,
  readFile,
  stat,
  writeFile,
} from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { MODULE_PATH } from "../domain.js";
import type { ModuleDraft } from "../modules.js";
import { MdcError } from "./client.js";
import { sha256 } from "./goal-file.js";
import type { ModuleRecord } from "./workspace.js";

/** LeanGround's limits on one contribution (docs/mathdoc.md §2.6 module decision 6). */
const MAX_FILES = 64;
const MAX_FILE_BYTES = 2 * 1024 * 1024;

/** The files a goal directory starts with: statement files, then shared modules (read only). */
export function startingFiles(draft: ModuleDraft) {
  const files = new Map<string, string>();
  const shared: Record<string, string> = {};
  for (const s of draft.shared)
    for (const [path, text] of Object.entries(s.files)) {
      files.set(path, text);
      shared[path] = sha256(text);
    }
  // The root's statement file is copied to Work/Main.lean: a proof restates the theorem in its
  // own module, so it never shares a module name with the statement it is connected to.
  const statement =
    draft.statement.from === "node"
      ? { "Work/Main.lean": Object.values(draft.statement.files)[0] }
      : draft.statement.files;
  for (const [path, text] of Object.entries(statement)) {
    if (files.has(path))
      throw new MdcError(
        "module_name_conflict",
        `${path} is both a statement file and a shared module`,
      );
    files.set(path, text);
  }
  return { files, shared, statement: Object.keys(statement).sort() };
}

/** GOAL.md: what to prove here and how the directory is submitted. */
export function goalNotes(draft: ModuleDraft, statementPaths: string[]) {
  const c = draft.statement.const;
  const sharedPaths = draft.shared.flatMap((s) => Object.keys(s.files)).sort();
  const negates = draft.statement.negates;
  const where =
    draft.statement.from === "node"
      ? `It is stated in ${statementPaths.map((p) => `\`${p}\``).join(", ")}, a copy of the project's root statement.`
      : negates
        ? `It is stated at the end of \`${negates.path}\`: the negation of goal \`${negates.goal}\`, ` +
          "whose statement is the rest of these files. A complete proof marks that goal false and refutes the decompositions using it; " +
          "a decomposition of the negation does not."
        : `It is stated in ${statementPaths.map((p) => `\`${p}\``).join(", ")}, ` +
          `the decomposition ${draft.statement.certificate?.slice(0, 12)} that introduced this goal (other theorems in it can be deleted).`;
  const lines = [
    `# ${c}`,
    "",
    `Goal \`${draft.goal}\` of project \`${draft.project}\` (status: ${draft.status}).`,
    "",
    `Prove the theorem \`${c}\`. ${where}`,
    "Keep its name and statement: LeanGround connects your theorem to the goal by the kernel, so a different statement is refused.",
    "",
    "Every `.lean` file in this directory is your contribution, written in any Lean you like, except:",
    "",
    ...(sharedPaths.length
      ? [
          "- the shared modules, which you may import but not change:",
          ...sharedPaths.map((p) => `  - \`${p}\``),
        ]
      : ["- (this goal uses no shared modules)"]),
    "- files under directories starting with `.`.",
    "",
    "Commands (run them here or give this directory's path):",
    "",
    "- `mdc check .` compiles the files on the server and checks them against this goal; nothing is recorded.",
    `- \`mdc prove .\` submits a complete proof: nothing \`${c}\` uses may be \`sorry\`.`,
    `- \`mdc split . -m "what the split does"\` submits a decomposition: the theorems \`${c}\` uses whose whole proof is \`sorry\` become new goals.`,
    "  Their statements may use only the base and the shared modules. Definitions they need go into new files you name with",
    "  `--shared FILE` (repeatable); those are registered as shared modules before the decomposition is checked.",
    `- \`--root NAME\` submits another theorem than \`${c}\` as the root (it must state the same goal).`,
    "- `mdc done .` ends your lease; `mdc note . -m TEXT` leaves what you tried for others.",
  ];
  if (draft.notes.length)
    lines.push(
      "",
      "## Notes on this goal (newest first)",
      "",
      ...draft.notes.map(
        (n) => `- ${n.at} ${n.actor}: ${n.text.replace(/\n/g, " ")}`,
      ),
    );
  return `${lines.join("\n")}\n`;
}

/** Write a goal directory; it must not exist yet (mdc never overwrites). */
export async function writeGoalDir(dir: string, draft: ModuleDraft) {
  try {
    await stat(dir);
    throw new MdcError("file_exists", `${dir} exists; mdc never overwrites it`);
  } catch (e) {
    if (e instanceof MdcError) throw e;
  }
  const start = startingFiles(draft);
  await mkdir(dir, { recursive: true });
  for (const [path, text] of start.files) {
    const file = join(dir, ...path.split("/"));
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, text, { flag: "wx" });
    if (Object.hasOwn(start.shared, path)) await chmod(file, 0o444);
  }
  await writeFile(join(dir, "GOAL.md"), goalNotes(draft, start.statement), {
    flag: "wx",
  });
  return start;
}

async function leanFiles(dir: string): Promise<string[]> {
  const out: string[] = [];
  const walk = async (d: string) => {
    for (const e of await readdir(d, { withFileTypes: true })) {
      if (e.name.startsWith(".")) continue;
      const full = join(d, e.name);
      if (e.isDirectory()) await walk(full);
      else if (e.isFile() && e.name.endsWith(".lean"))
        out.push(relative(dir, full).split(sep).join("/"));
    }
  };
  await walk(dir);
  return out.sort();
}

/**
 * The contribution in a goal directory: every `.lean` file except the shared modules (which
 * must be unchanged) and the files named as new shared modules.
 */
export async function contribution(
  dir: string,
  record: ModuleRecord,
  newShared: string[] = [],
) {
  const base = resolve(dir);
  const named = new Set<string>();
  for (const f of newShared) {
    const rel = relative(base, resolve(f)).split(sep).join("/");
    if (rel.startsWith("..") || !rel.endsWith(".lean"))
      throw new MdcError(
        "invalid_path",
        `--shared ${f} is not a .lean file in ${record.file}`,
      );
    named.add(rel);
  }
  const files: Record<string, string> = {};
  const shared: Record<string, string> = {};
  const all = await leanFiles(base);
  for (const path of all) {
    if (!MODULE_PATH.test(path))
      throw new MdcError(
        "invalid_path",
        `${path}: every directory and file name must be a Lean identifier (A/B.lean)`,
      );
    const text = (
      await readFile(join(base, ...path.split("/")), "utf8")
    ).replace(/\r\n/g, "\n");
    if (Buffer.byteLength(text, "utf8") > MAX_FILE_BYTES)
      throw new MdcError("file_too_large", `${path} is over 2 MB`);
    if (Object.hasOwn(record.shared.files, path)) {
      if (sha256(text) !== record.shared.files[path])
        throw new MdcError(
          "shared_module_modified",
          `${path} is a shared module and must stay as pulled; put changes in your own files`,
        );
      continue;
    }
    (named.has(path) ? shared : files)[path] = text;
  }
  for (const path of Object.keys(record.shared.files))
    if (!all.includes(path))
      throw new MdcError(
        "shared_module_missing",
        `${path} is a shared module this goal was pulled with; restore it`,
      );
  for (const path of named)
    if (!Object.hasOwn(shared, path))
      throw new MdcError(
        "invalid_path",
        `--shared ${path} is not a file of the contribution`,
      );
  if (!Object.keys(files).length)
    throw new MdcError(
      "no_files",
      `${record.file} has no .lean file of its own`,
    );
  if (Object.keys(files).length > MAX_FILES)
    throw new MdcError("too_many_files", `at most ${MAX_FILES} files`);
  return { files, newShared: Object.keys(shared).length ? shared : undefined };
}

/** Lean's diagnostics as `file:line:col: severity: message` lines. */
export function messages(diagnostics: unknown): string {
  if (!Array.isArray(diagnostics)) return "";
  return diagnostics
    .map(
      (d: {
        file?: string;
        line?: number;
        column?: number;
        severity?: string;
        message?: string;
      }) =>
        `${d.file ?? "?"}:${d.line ?? "?"}:${d.column ?? "?"}: ${d.severity ?? "error"}: ${d.message ?? ""}`,
    )
    .join("\n");
}
