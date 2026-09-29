// Conservative scanner for a node's Lean block (plan §7.3; contract
// mathdoc.declaration-binding.v1, decision B). It never elaborates: top-level commands
// start at column 0, anything unrecognised is rejected with a machine-readable reason, and
// LeanGround's own checks stay final. Positions refer to the original source.

export type DeclKind =
  | "theorem"
  | "lemma"
  | "def"
  | "abbrev"
  | "noncomputable_def"
  | "structure"
  | "inductive";
export const DEFINITION_KINDS: DeclKind[] = [
  "def",
  "abbrev",
  "noncomputable_def",
  "structure",
  "inductive",
];

export interface Context {
  local_options?: string[];
  universes?: string[];
  opens?: string[];
  variables?: string[];
}
export interface Decl {
  kind: DeclKind;
  name: string;
  start: number;
  end: number;
  /** Theorem signature; absent for definitions. */
  binders?: string;
  type?: string;
  /** The whole proof body is `sorry` / `by sorry`. */
  sorry?: boolean;
}
export interface Rejection {
  reason:
    | "unsupported_declaration"
    | "ambiguous_binding"
    | "multiple_conclusions"
    | "mixed_declarations"
    | "local_context_mismatch"
    | "premise_name_collision"
    | "definition_name_taken"
    | "unregistered_statement_definition"
    | "missing_environment_instance";
  details: string;
}
export interface Scan {
  source: string;
  /** Items outside declarations: import lines and context commands, in order. */
  prelude: { start: number; end: number }[];
  imports: string[];
  context: Context;
  decls: Decl[];
  rejection?: Rejection;
}

/**
 * Replace comments (and, unless `keepStrings`, string contents) with spaces, keeping
 * newlines and every offset. Nested block comments follow Lean's lexer.
 */
export function mask(source: string, keepStrings = false): string {
  const out = source.split("");
  let i = 0;
  const blank = (from: number, to: number) => {
    for (let k = from; k < to; k++) if (out[k] !== "\n") out[k] = " ";
  };
  while (i < source.length) {
    if (source.startsWith("--", i)) {
      const end = source.indexOf("\n", i);
      const stop = end < 0 ? source.length : end;
      blank(i, stop);
      i = stop;
    } else if (source.startsWith("/-", i)) {
      let depth = 0;
      let k = i;
      while (k < source.length) {
        if (source.startsWith("/-", k)) {
          depth++;
          k += 2;
        } else if (source.startsWith("-/", k)) {
          depth--;
          k += 2;
          if (depth === 0) break;
        } else k++;
      }
      blank(i, k);
      i = k;
    } else if (source[i] === '"') {
      let k = i + 1;
      while (k < source.length && source[k] !== '"')
        k += source[k] === "\\" ? 2 : 1;
      if (!keepStrings) blank(i + 1, Math.min(k, source.length));
      i = k + 1;
    } else i++;
  }
  return out.join("");
}

const OPEN = "([{⦃⟨";
const CLOSE = ")]}⦄⟩";

/** Index of `:=` at bracket depth 0 at or after `from`, or -1. */
function assignment(masked: string, from: number, to: number): number {
  let depth = 0;
  for (let i = from; i < to; i++) {
    const c = masked[i];
    if (OPEN.includes(c)) depth++;
    else if (CLOSE.includes(c)) depth--;
    else if (depth === 0 && c === ":" && masked[i + 1] === "=") return i;
  }
  return -1;
}

const collapse = (text: string) => text.replace(/\s+/g, " ").trim();

function signature(
  masked: string,
  afterName: number,
  end: number,
  comments: string,
): { binders: string; type: string; body: number } | null {
  let i = afterName;
  const binderStart = i;
  for (;;) {
    while (i < end && /\s/.test(masked[i])) i++;
    if (i < end && "([{⦃".includes(masked[i])) {
      let depth = 0;
      for (; i < end; i++) {
        if (OPEN.includes(masked[i])) depth++;
        else if (CLOSE.includes(masked[i]) && --depth === 0) break;
      }
      if (i >= end) return null;
      i++;
    } else break;
  }
  if (masked[i] !== ":" || masked[i + 1] === "=") return null;
  const binders = collapse(comments.slice(binderStart, i));
  const assign = assignment(masked, i + 1, end);
  if (assign < 0) return null;
  const type = collapse(comments.slice(i + 1, assign));
  if (!type) return null;
  return { binders, type, body: assign + 2 };
}

const NAME = /^[^\s([{⦃:]+/;

/** Split a Lean block into its prelude (imports, context commands) and declarations. */
export function scan(source: string): Scan {
  const masked = mask(source);
  const comments = mask(source, true);
  const starts: number[] = [];
  for (let i = 0; ;) {
    if (i < masked.length && !/\s/.test(masked[i])) starts.push(i);
    const newline = masked.indexOf("\n", i);
    if (newline < 0) break;
    i = newline + 1;
  }
  const result: Scan = {
    source,
    prelude: [],
    imports: [],
    context: {},
    decls: [],
  };
  const reject = (reason: Rejection["reason"], details: string) => {
    result.rejection ??= { reason, details };
  };
  const push = (key: keyof Context, value: string) =>
    (result.context[key] ??= []).push(value);
  for (const [n, start] of starts.entries()) {
    const end = starts[n + 1] ?? source.length;
    const text = masked.slice(start, end);
    const words = text.trim().split(/\s+/);
    const line = collapse(comments.slice(start, end));
    const rest = (k: number) => collapse(line.split(/\s+/).slice(k).join(" "));
    const [first, second] = words;
    if (first === "import") {
      result.imports.push(rest(1));
      result.prelude.push({ start, end });
      continue;
    }
    if (first === "open" || first === "universe" || first === "variable") {
      if (text.includes(" in ") || /\bin\s*$/.test(text))
        reject("unsupported_declaration", `\`${first} … in\` is not supported`);
      push(
        first === "open"
          ? "opens"
          : first === "universe"
            ? "universes"
            : "variables",
        rest(1),
      );
      result.prelude.push({ start, end });
      continue;
    }
    if (first === "set_option") {
      push("local_options", line);
      result.prelude.push({ start, end });
      continue;
    }
    let kind: DeclKind | null = null;
    let nameAt = 1;
    if (first === "noncomputable" && second === "def") {
      kind = "noncomputable_def";
      nameAt = 2;
    } else if (
      ["theorem", "lemma", "def", "abbrev", "structure", "inductive"].includes(
        first,
      )
    )
      kind = first as DeclKind;
    if (!kind) {
      const what = ["instance", "class"].includes(first)
        ? `\`${first}\` belongs in the proof environment's fixed context`
        : `\`${first}\` is not a supported top-level command`;
      reject("unsupported_declaration", what);
      continue;
    }
    // Offset of the name in the original text.
    let at = start;
    for (let k = 0; k < nameAt; k++) {
      while (/\s/.test(masked[at])) at++;
      while (at < end && !/\s/.test(masked[at])) at++;
    }
    while (at < end && /\s/.test(masked[at])) at++;
    const name = NAME.exec(masked.slice(at, end))?.[0];
    if (!name) {
      reject("ambiguous_binding", `a \`${first}\` without a name`);
      continue;
    }
    const decl: Decl = { kind, name, start, end };
    if (kind === "theorem" || kind === "lemma") {
      const sig = signature(masked, at + name.length, end, comments);
      if (!sig) {
        reject(
          "ambiguous_binding",
          `cannot read the statement of \`${name}\` (expected \`theorem ${name} binders : type := proof\`)`,
        );
        continue;
      }
      decl.binders = sig.binders;
      decl.type = sig.type;
      decl.sorry = ["sorry", "by sorry"].includes(
        collapse(masked.slice(sig.body, end)),
      );
    }
    result.decls.push(decl);
  }
  return result;
}

/** The proposition LeanGround resolves for a theorem: binders become a ∀. */
export function proposition(d: Decl): string {
  return d.binders ? `∀ ${d.binders}, ${d.type}` : d.type!;
}
/** A premise placeholder with the same statement, on one line. */
export function placeholder(d: Decl): string {
  return `theorem ${d.name}${d.binders ? ` ${d.binders}` : ""} : ${d.type} := sorry\n`;
}
/** The declarations only: imports and context commands removed. */
export function body(s: Scan): string {
  let out = "";
  for (const d of s.decls) out += s.source.slice(d.start, d.end);
  return out.endsWith("\n") || !out ? out : `${out}\n`;
}
/** Declaration text without its sorry placeholders. */
export function withoutPlaceholders(source: string): string {
  const s = scan(source);
  return s.decls
    .filter((d) => !d.sorry)
    .map((d) => source.slice(d.start, d.end))
    .join("");
}
/** The prelude lines (imports, context commands) of a block, verbatim. */
export function preludeText(s: Scan): string {
  return s.prelude.map((p) => s.source.slice(p.start, p.end)).join("");
}

export type Role =
  | {
      role: "definition";
      source: "syntax_scan" | "explicit_metadata";
      names: string[];
    }
  | {
      role: "theorem";
      source: "syntax_scan" | "explicit_metadata";
      conclusion: Decl;
    }
  | { role: "unsupported"; rejection: Rejection };

/**
 * Decision B: classify only obvious single-kind blocks by syntax; anything else needs
 * `lean_role` / `lean_conclusion` metadata, and is rejected when still unclear.
 */
export function classify(s: Scan, metadata: Record<string, string>): Role {
  if (s.rejection) return { role: "unsupported", rejection: s.rejection };
  const theorems = s.decls.filter(
    (d) => d.kind === "theorem" || d.kind === "lemma",
  );
  const definitions = s.decls.filter((d) => DEFINITION_KINDS.includes(d.kind));
  const declared = metadata.lean_role;
  const conclusion = metadata.lean_conclusion;
  const unsupported = (reason: Rejection["reason"], details: string): Role => ({
    role: "unsupported",
    rejection: { reason, details },
  });
  if (declared !== undefined && !["definition", "theorem"].includes(declared))
    return unsupported(
      "ambiguous_binding",
      `lean_role must be definition or theorem, not ${JSON.stringify(declared)}`,
    );
  if (!s.decls.length)
    return unsupported(
      "ambiguous_binding",
      "the Lean block has no declaration",
    );
  if (declared === "definition" || (!declared && !theorems.length)) {
    if (theorems.length)
      return unsupported(
        "mixed_declarations",
        `lean_role is definition but the block also proves ${theorems.map((t) => t.name).join(", ")}`,
      );
    return {
      role: "definition",
      source: declared ? "explicit_metadata" : "syntax_scan",
      names: definitions.map((d) => d.name),
    };
  }
  if (conclusion !== undefined) {
    const chosen = theorems.find((t) => t.name === conclusion);
    if (!chosen)
      return unsupported(
        "ambiguous_binding",
        `lean_conclusion ${conclusion} is not a theorem in this block`,
      );
    return { role: "theorem", source: "explicit_metadata", conclusion: chosen };
  }
  if (definitions.length)
    return unsupported(
      "mixed_declarations",
      "definitions and theorems in one block: set lean_role and lean_conclusion, or split the node",
    );
  if (theorems.length > 1)
    return unsupported(
      "multiple_conclusions",
      `several theorems (${theorems.map((t) => t.name).join(", ")}): set lean_conclusion`,
    );
  if (declared === "theorem" || !declared)
    return {
      role: "theorem",
      source: declared ? "explicit_metadata" : "syntax_scan",
      conclusion: theorems[0],
    };
  return unsupported("ambiguous_binding", "unclassified Lean block");
}

/** Exact comparison of the block's context commands with the fixed environment context. */
export function contextMismatch(
  required: Context,
  fixed: Record<string, string[] | undefined>,
): string | null {
  for (const [key, lines] of Object.entries(required) as [
    keyof Context,
    string[],
  ][]) {
    const want = fixed[key] ?? [];
    if (
      lines.length !== want.length ||
      lines.some((line, i) => line !== want[i])
    )
      return `${key} ${JSON.stringify(lines)} differs from the proof environment's ${JSON.stringify(want)}`;
  }
  return null;
}
