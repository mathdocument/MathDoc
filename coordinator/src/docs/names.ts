// Names, identifiers and canonical JSON shared by the document backend.
// Each rule mirrors the legacy Rust backend (src/config.rs, src/store.rs at a4d61e3)
// so that both backends accept and reject the same data while they share a database.
import { createHash } from "node:crypto";

export class DocError extends Error {
  constructor(
    message: string,
    readonly status = 422,
  ) {
    super(message);
  }
}

export function digest(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** config::validate_name: ASCII letters, digits, hyphens and underscores. */
export function validateName(name: string): void {
  if (!/^[A-Za-z0-9_-]+$/.test(name))
    throw new DocError(
      "database and branch names allow letters, digits, hyphens and underscores",
    );
}

export function projectParts(project: string): [string, string] {
  const slash = project.indexOf("/");
  if (slash < 0)
    throw new DocError("use DATABASE/BRANCH, for example etp/main");
  const database = project.slice(0, slash);
  const branch = project.slice(slash + 1);
  validateName(database);
  validateName(branch);
  return [database, branch];
}

/** uuid::Uuid::parse_str(x)?.to_string() == x: canonical lowercase hyphenated. */
export function isCanonicalUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
    value,
  );
}

/** Any spelling uuid::Uuid::parse_str accepts, normalized; null otherwise. */
export function parseUuid(value: string): string | null {
  let v = value;
  if (/^urn:uuid:/i.test(v)) v = v.slice(9);
  if (v.startsWith("{") && v.endsWith("}")) v = v.slice(1, -1);
  const hex = /^[0-9a-fA-F]{32}$/.test(v)
    ? v
    : /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(
          v,
        )
      ? v.replaceAll("-", "")
      : null;
  if (hex === null) return null;
  const h = hex.toLowerCase();
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

// Rust char predicates.
const WHITE = /^\p{White_Space}+|\p{White_Space}+$/gu;
export const rustTrim = (s: string) => s.replace(WHITE, "");
export const hasControl = (s: string) => /\p{Cc}/u.test(s);
export const hasWhitespace = (s: string) => /\p{White_Space}/u.test(s);

function plainModulePart(part: string): boolean {
  return /^[\p{Alphabetic}_][\p{Alphabetic}\p{N}_']*$/u.test(part);
}

/** store::module_parts: Lean's quoted identifiers can contain dots. */
export function moduleParts(input: string): string[] {
  let module = input;
  const parts: string[] = [];
  while (module.length > 0) {
    let part: string;
    let rest: string;
    if (module.startsWith("«")) {
      const quoted = module.slice(1);
      const end = quoted.indexOf("»");
      if (end < 0) throw new DocError("unclosed Lean module identifier");
      part = quoted.slice(0, end);
      rest = quoted.slice(end + 1);
    } else {
      const dot = module.indexOf(".");
      part = dot < 0 ? module : module.slice(0, dot);
      rest = dot < 0 ? "" : module.slice(dot + 1);
      if (!plainModulePart(part))
        throw new DocError("invalid unquoted Lean module identifier");
      parts.push(part);
      if (rest === "") {
        if (module.endsWith("."))
          throw new DocError("empty Lean module identifier");
        break;
      }
      module = rest;
      continue;
    }
    if (
      part === "" ||
      part === "." ||
      part === ".." ||
      /[\p{Cc}/\\«»]/u.test(part)
    )
      throw new DocError("unsafe Lean module identifier");
    parts.push(part);
    if (rest === "") break;
    if (!rest.startsWith(".") || rest.length === 1)
      throw new DocError("invalid Lean module separator");
    module = rest.slice(1);
  }
  if (
    parts.length === 0 ||
    parts.some((p) => p.startsWith(".")) ||
    parts[0] === "lakefile"
  )
    throw new DocError("unsafe or empty Lean module name");
  return parts;
}

/** store::module_file joined with "/" (the path string the Rust backend compares). */
export function moduleFile(module: string, extension: string): string {
  const parts = moduleParts(module);
  const last = parts.pop()!;
  return [...parts, `${last}.${extension}`].join("/");
}

/** Rust `String` ordering: UTF-8 byte order (JavaScript sorts UTF-16 code units). */
export function byteOrder(a: string, b: string): number {
  return Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8"));
}

/**
 * serde_json serialization of an already canonical value. Objects are emitted in the
 * given key order; callers pass field order explicitly. JSON.stringify of strings
 * matches serde_json's escaping (quotes, backslashes, control characters as \uXXXX).
 */
export type Canonical =
  | string
  | number
  | boolean
  | null
  | Canonical[]
  | { readonly entries: [string, Canonical][] };
export function obj(entries: [string, Canonical][]): Canonical {
  return { entries };
}
export function serialize(value: Canonical): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(serialize).join(",")}]`;
  return `{${value.entries.map(([k, v]) => `${JSON.stringify(k)}:${serialize(v)}`).join(",")}}`;
}
/** A BTreeMap<String, String> in Rust order. */
export function sortedMap(map: Record<string, string>): Canonical {
  return obj(
    Object.keys(map)
      .sort(byteOrder)
      .map((k) => [k, map[k]]),
  );
}
