// Document model of the legacy backend (src/store.rs, src/latex/project.rs at a4d61e3).
import {
  DocError,
  byteOrder,
  digest,
  hasControl,
  hasWhitespace,
  isCanonicalUuid,
  moduleFile,
  moduleParts,
  obj,
  rustTrim,
  serialize,
  sortedMap,
} from "./names.js";
import { parseToml, type TomlTable, type TomlValue } from "./toml.js";

export const BLOCK_TYPES = ["text", "lean", "rocq", "latex"] as const;

export interface Block {
  srctype: string;
  content: string;
  metadata: Record<string, string>;
}
export interface Node {
  fnode: string;
  title: string;
  module: string;
  depens: string[];
  blocks: Block[];
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** serde(deny_unknown_fields) plus field types; `optional` fields may be absent. */
function strictFields(
  value: unknown,
  what: string,
  required: string[],
  optional: string[] = [],
): Record<string, unknown> {
  if (!isRecord(value)) throw new DocError(`invalid ${what}`);
  for (const key of Object.keys(value))
    if (!required.includes(key) && !optional.includes(key))
      throw new DocError(`unknown field \`${key}\` in ${what}`);
  for (const key of required)
    if (!(key in value))
      throw new DocError(`missing field \`${key}\` in ${what}`);
  return value;
}
function string(value: unknown, what: string): string {
  if (typeof value !== "string") throw new DocError(`invalid ${what}`);
  return value;
}
function stringMap(value: unknown, what: string): Record<string, string> {
  if (!isRecord(value)) throw new DocError(`invalid ${what}`);
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(value))
    Object.defineProperty(out, k, {
      value: string(v, what),
      enumerable: true,
      writable: true,
      configurable: true,
    });
  return out;
}

export function parseBlock(value: unknown): Block {
  const v = strictFields(value, "block", ["srctype", "content"], ["metadata"]);
  return {
    srctype: string(v.srctype, "block.srctype"),
    content: string(v.content, "block.content"),
    metadata:
      v.metadata === undefined ? {} : stringMap(v.metadata, "block.metadata"),
  };
}

/** Wire form (import bundles, write-back): serde(default) for depens and blocks. */
export function parseNode(value: unknown): Node {
  const v = strictFields(
    value,
    "node",
    ["fnode", "title", "module"],
    ["depens", "blocks"],
  );
  const node: Node = {
    fnode: string(v.fnode, "node.fnode"),
    title: string(v.title, "node.title"),
    module: string(v.module, "node.module"),
    depens:
      v.depens === undefined
        ? []
        : arrayOf(v.depens, (d) => string(d, "node.depens")),
    blocks: v.blocks === undefined ? [] : arrayOf(v.blocks, parseBlock),
  };
  validateNode(node);
  return node;
}
function arrayOf<T>(value: unknown, item: (v: unknown) => T): T[] {
  if (!Array.isArray(value)) throw new DocError("expected an array");
  return value.map(item);
}

export function validateNode(node: Node): void {
  if (!isCanonicalUuid(node.fnode))
    throw new DocError(
      "node identity must be a canonical lowercase hyphenated UUID",
    );
  if (
    rustTrim(node.title) === "" ||
    node.title !== rustTrim(node.title) ||
    hasControl(node.title)
  )
    throw new DocError(
      "name must be nonempty, trimmed and contain no control characters",
    );
  moduleParts(node.module);
  const types = new Set<string>();
  for (const block of node.blocks) {
    if (
      !(BLOCK_TYPES as readonly string[]).includes(block.srctype) ||
      types.has(block.srctype)
    )
      throw new DocError(
        "blocks must have distinct types: text, lean, rocq, latex",
      );
    types.add(block.srctype);
  }
  const deps = new Set<string>();
  for (const dep of node.depens) {
    if (dep === node.fnode || !isCanonicalUuid(dep) || deps.has(dep))
      throw new DocError(
        "dependencies must be distinct UUIDs other than the node itself",
      );
    deps.add(dep);
  }
}

export function newNode(title: string, moduleRoot: string): Node {
  const fnode = crypto.randomUUID();
  const node = {
    fnode,
    title,
    module: `${moduleRoot}.N_${fnode.replaceAll("-", "")}`,
    depens: [],
    blocks: [],
  };
  validateNode(node);
  return node;
}

function blocksCanonical(blocks: Block[]) {
  return blocks.map((b) =>
    obj([
      ["srctype", b.srctype],
      ["content", b.content],
      ["metadata", sortedMap(b.metadata)],
    ]),
  );
}
function nodeCanonical(node: Node, sortDepens: boolean) {
  const depens = sortDepens ? [...node.depens].sort(byteOrder) : node.depens;
  return obj([
    ["fnode", node.fnode],
    ["title", node.title],
    ["module", node.module],
    ["depens", depens],
    ["blocks", blocksCanonical(node.blocks)],
  ]);
}
/** Node::revision: SHA-256 of serde_json with sorted dependencies and metadata. */
export function nodeRevision(node: Node): string {
  return digest(serialize(nodeCanonical(node, true)));
}
/** serde_json::to_value(Node): the export/import wire form. */
export function nodeJson(node: Node): string {
  return serialize(nodeCanonical(node, false));
}
export function nodeSource(node: Node, language: string): string | undefined {
  return node.blocks.find((b) => b.srctype === language)?.content;
}

/** Node::document: the TerminusDB document. */
export function nodeDocument(node: Node) {
  return {
    "@id": `Node/${node.fnode}`,
    "@type": "Node",
    fnode: node.fnode,
    title: node.title,
    module: node.module,
    depens: node.depens.map((id) => `Node/${id}`),
    blocks: serialize(blocksCanonical(node.blocks)),
  };
}
export function nodeFromDocument(doc: Record<string, unknown>): Node {
  const field = (name: string) => {
    if (typeof doc[name] !== "string")
      throw new DocError(`invalid Node.${name}`);
    return doc[name] as string;
  };
  const depens = Array.isArray(doc.depens) ? doc.depens : [];
  let blocks: unknown;
  try {
    blocks = JSON.parse(field("blocks"));
  } catch {
    throw new DocError("invalid Node.blocks");
  }
  const node: Node = {
    fnode: field("fnode"),
    title: field("title"),
    module: field("module"),
    depens: depens.map((v) => {
      if (typeof v !== "string") throw new DocError("invalid dependency link");
      return v.split("/").pop()!;
    }),
    blocks: arrayOf(blocks, parseBlock),
  };
  validateNode(node);
  return node;
}

// ---- Lean project (legacy Lake configuration, preserved unmodified) -------------

export interface LeanProject {
  toolchain: string;
  lakefile: string;
  manifest: string | null;
  lakefile_name?: string;
  module_root?: string;
  files: Record<string, string>;
}
export const DEFAULT_LEAN_PROJECT: LeanProject = {
  toolchain: "leanprover/lean4:v4.33.1",
  lakefile:
    'name = "MathDoc"\nversion = "0.1.0"\n\n[[lean_lib]]\nname = "Lib"\n',
  manifest: null,
  files: {},
};
export function parseLeanProject(value: unknown): LeanProject {
  const v = strictFields(
    value,
    "Lean project",
    ["toolchain", "lakefile"],
    ["manifest", "lakefile_name", "module_root", "files"],
  );
  const optional = (k: string) =>
    v[k] === undefined || v[k] === null
      ? undefined
      : string(v[k], `project.${k}`);
  return {
    toolchain: string(v.toolchain, "project.toolchain"),
    lakefile: string(v.lakefile, "project.lakefile"),
    manifest: optional("manifest") ?? null,
    ...(optional("lakefile_name") !== undefined
      ? { lakefile_name: optional("lakefile_name") }
      : {}),
    ...(optional("module_root") !== undefined
      ? { module_root: optional("module_root") }
      : {}),
    files: v.files === undefined ? {} : stringMap(v.files, "project.files"),
  };
}
/** serde_json::to_string(LeanProject), field order and skip rules included. */
export function leanProjectJson(p: LeanProject): string {
  const entries: [string, import("./names.js").Canonical][] = [
    ["toolchain", p.toolchain],
    ["lakefile", p.lakefile],
    ["manifest", p.manifest],
  ];
  if (p.lakefile_name !== undefined)
    entries.push(["lakefile_name", p.lakefile_name]);
  if (p.module_root !== undefined) entries.push(["module_root", p.module_root]);
  if (Object.keys(p.files).length) entries.push(["files", sortedMap(p.files)]);
  return serialize(obj(entries));
}
export const moduleRoot = (p: LeanProject) => p.module_root ?? "Lib";
const lakefileName = (p: LeanProject) => p.lakefile_name ?? "lakefile.toml";

export function validateModules(p: LeanProject, nodes: Iterable<Node>): void {
  for (const node of nodes)
    if (Object.hasOwn(p.files, moduleFile(node.module, "lean")))
      throw new DocError(
        `managed module ${node.module} collides with a project file`,
      );
}

/** LeanProject::validate. The TOML checks use the fail-closed subset reader. */
export function validateLeanProject(p: LeanProject): void {
  if (
    !p.toolchain.startsWith("leanprover/lean4:v") ||
    hasWhitespace(p.toolchain)
  )
    throw new DocError(
      "pin a Lean toolchain release, such as leanprover/lean4:v4.33.1",
    );
  moduleParts(moduleRoot(p));
  let config: TomlTable = {};
  const name = lakefileName(p);
  if (name === "lakefile.toml") config = parseToml(p.lakefile);
  else if (name === "lakefile.lean") {
    if (p.manifest === null)
      throw new DocError("native Lean projects require a pinned Lake manifest");
  } else
    throw new DocError("lakefile_name must be lakefile.toml or lakefile.lean");
  const tables = (v: TomlValue | undefined) =>
    Array.isArray(v)
      ? v.filter(
          (x): x is TomlTable => typeof x === "object" && !Array.isArray(x),
        )
      : null;
  if (
    name === "lakefile.toml" &&
    !(tables(config.lean_lib) ?? []).some((l) => l.name === moduleRoot(p))
  )
    throw new DocError(
      `Lake project must declare the ${moduleRoot(p)} lean_lib`,
    );
  for (const key of ["srcDir", "buildDir", "leanLibDir"])
    if (Object.hasOwn(config, key))
      throw new DocError(`custom ${key} is not supported in managed projects`);
  for (const path of Object.keys(p.files))
    if (
      path === "" ||
      path
        .split("/")
        .some(
          (part) =>
            part === "" || part.startsWith(".") || /[\p{Cc}\\]/u.test(part),
        ) ||
      [
        "lean-toolchain",
        "lakefile.toml",
        "lakefile.lean",
        "lake-manifest.json",
      ].includes(path)
    )
      throw new DocError(`unsafe or reserved project file path: ${path}`);
  const required = Array.isArray(config.require)
    ? (config.require as TomlValue[])
    : null;
  if (required && required.length && p.manifest === null)
    throw new DocError(
      "external libraries require a committed lake-manifest.json with pinned Git revisions",
    );
  if (
    (required ?? []).some(
      (r) =>
        typeof r === "object" && !Array.isArray(r) && Object.hasOwn(r, "path"),
    )
  )
    throw new DocError(
      "publish local libraries to Git and pin them; mutable path dependencies are unsupported",
    );
  if (p.manifest !== null) {
    let manifest: unknown;
    try {
      manifest = JSON.parse(p.manifest);
    } catch (e) {
      throw new DocError(String((e as Error).message));
    }
    const packages =
      isRecord(manifest) && Array.isArray(manifest.packages)
        ? manifest.packages
        : null;
    if (!packages) throw new DocError("Lake manifest must contain packages");
    if (
      isRecord(manifest) &&
      typeof manifest.packagesDir === "string" &&
      manifest.packagesDir !== ".lake/packages"
    )
      throw new DocError("Lake packagesDir must be .lake/packages");
    for (const pkg of packages) {
      const rev = isRecord(pkg) && typeof pkg.rev === "string" ? pkg.rev : "";
      if (
        !isRecord(pkg) ||
        pkg.type !== "git" ||
        !/^[0-9a-fA-F]{40}$/.test(rev)
      )
        throw new DocError(
          "every library must be locked to a full Git commit in the Lake manifest",
        );
    }
    for (const r of required ?? []) {
      const name = isRecord(r) && typeof r.name === "string" ? r.name : null;
      if (name === null) throw new DocError("Lake dependency needs a name");
      if (!packages.some((pkg) => isRecord(pkg) && pkg.name === name))
        throw new DocError(`library ${name} is missing from the Lake manifest`);
    }
  }
}

// ---- LaTeX project ---------------------------------------------------------------

export interface LatexProject {
  preamble_name: string;
  preamble: string;
  bibliography_name: string;
  bibliography: string;
}
export const DEFAULT_LATEX_PROJECT: LatexProject = {
  preamble_name: "preamble.tex",
  preamble: "",
  bibliography_name: "references.bib",
  bibliography: "",
};
/** serde(default, deny_unknown_fields): every field may be omitted. */
export function parseLatexProject(value: unknown): LatexProject {
  const v = strictFields(
    value ?? {},
    "LaTeX project",
    [],
    Object.keys(DEFAULT_LATEX_PROJECT),
  );
  const out = { ...DEFAULT_LATEX_PROJECT };
  for (const key of Object.keys(out) as (keyof LatexProject)[])
    if (v[key] !== undefined) out[key] = string(v[key], `latex_project.${key}`);
  return out;
}
export function latexProjectJson(p: LatexProject): string {
  return serialize(
    obj([
      ["preamble_name", p.preamble_name],
      ["preamble", p.preamble],
      ["bibliography_name", p.bibliography_name],
      ["bibliography", p.bibliography],
    ]),
  );
}
export const latexProjectKey = (p: LatexProject) => digest(latexProjectJson(p));
export function validateLatexProject(p: LatexProject): void {
  for (const [name, extensions] of [
    [p.preamble_name, ["tex", "cls"]],
    [p.bibliography_name, ["bib"]],
  ] as const) {
    const dot = name.lastIndexOf(".");
    if (
      name === "" ||
      name.startsWith(".") ||
      /[\p{Cc}/\\]/u.test(name) ||
      dot < 0 ||
      !(extensions as readonly string[]).includes(name.slice(dot + 1))
    )
      throw new DocError(
        "LaTeX project files must be named .tex/.cls and .bib files without paths",
      );
  }
  if (Buffer.byteLength(p.preamble) > 2 * 1024 * 1024)
    throw new DocError("LaTeX preamble exceeds 2 MiB");
  if (Buffer.byteLength(p.bibliography) > 16 * 1024 * 1024)
    throw new DocError("bibliography exceeds 16 MiB");
}
