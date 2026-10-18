// A deliberately small, fail-closed TOML reader for Lake project files.
//
// The legacy backend validates lakefile.toml with a full TOML parser before writing it.
// This reader accepts a strict subset (comments, bare or quoted keys, [table],
// [[array-of-tables]], basic and literal strings, integers, booleans and single-line
// arrays of those). Anything else is rejected, so this backend may refuse a file the
// legacy backend would accept, but never accepts one it would reject.
import { DocError } from "./names.js";

export type TomlValue = string | number | boolean | TomlValue[] | TomlTable;
export interface TomlTable {
  [key: string]: TomlValue;
}

const unsupported = (why: string, line: number) =>
  new DocError(
    `unsupported lakefile.toml syntax on line ${line} (${why}); simplify the file or edit it with the legacy backend`,
  );

function readString(s: string, i: number, line: number): [string, number] {
  const quote = s[i];
  if (s.startsWith(quote.repeat(3), i))
    throw unsupported("multi-line string", line);
  let out = "";
  i++;
  while (i < s.length) {
    const c = s[i];
    if (c === quote) return [out, i + 1];
    if (/[\u0000-\u0008\u000a-\u001f\u007f]/.test(c))
      throw unsupported("control character in string", line);
    if (c === "\\" && quote === '"') {
      const e = s[i + 1];
      const simple: Record<string, string> = {
        '"': '"',
        "\\": "\\",
        n: "\n",
        t: "\t",
        r: "\r",
        b: "\b",
        f: "\f",
      };
      if (e in simple) {
        out += simple[e];
        i += 2;
        continue;
      }
      const width = e === "u" ? 4 : e === "U" ? 8 : 0;
      const hex = s.slice(i + 2, i + 2 + width);
      if (!width || !new RegExp(`^[0-9a-fA-F]{${width}}$`).test(hex))
        throw unsupported("invalid escape", line);
      const code = parseInt(hex, 16);
      if (code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff))
        throw unsupported("invalid escape", line);
      out += String.fromCodePoint(code);
      i += 2 + width;
      continue;
    }
    out += c;
    i++;
  }
  throw unsupported("unterminated string", line);
}

// Keys come from the file: never let "__proto__" reach a prototype setter.
function put(table: TomlTable, key: string, value: TomlValue) {
  Object.defineProperty(table, key, {
    value,
    enumerable: true,
    writable: true,
    configurable: true,
  });
}

function skipSpace(s: string, i: number) {
  while (s[i] === " " || s[i] === "\t") i++;
  return i;
}

function readKey(s: string, i: number, line: number): [string, number] {
  if (s[i] === '"' || s[i] === "'") return readString(s, i, line);
  const m = /^[A-Za-z0-9_-]+/.exec(s.slice(i));
  if (!m) throw unsupported("invalid key", line);
  return [m[0], i + m[0].length];
}

function readValue(s: string, i: number, line: number): [TomlValue, number] {
  const c = s[i];
  if (c === '"' || c === "'") return readString(s, i, line);
  if (c === "[") {
    const out: TomlValue[] = [];
    i = skipSpace(s, i + 1);
    while (s[i] !== "]") {
      if (i >= s.length) throw unsupported("multi-line array", line);
      const [value, next] = readValue(s, i, line);
      if (Array.isArray(value)) throw unsupported("nested array", line);
      out.push(value);
      i = skipSpace(s, next);
      if (s[i] === ",") i = skipSpace(s, i + 1);
      else if (s[i] !== "]") throw unsupported("invalid array", line);
    }
    return [out, i + 1];
  }
  if (c === "{") throw unsupported("inline table", line);
  const m = /^(true|false|[+-]?(0|[1-9](_?[0-9])*))(?=[ \t#]|$)/.exec(
    s.slice(i),
  );
  if (!m) throw unsupported("unsupported value", line);
  const text = m[1];
  const value =
    text === "true"
      ? true
      : text === "false"
        ? false
        : Number(text.replaceAll("_", ""));
  if (typeof value === "number" && !Number.isSafeInteger(value))
    throw unsupported("integer out of range", line);
  return [value, i + text.length];
}

function endOfLine(s: string, i: number, line: number) {
  i = skipSpace(s, i);
  if (i < s.length && s[i] !== "#") throw unsupported("trailing content", line);
}

export function parseToml(text: string): TomlTable {
  const root: TomlTable = {};
  let current = root;
  const tables = new Set<string>();
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  lines.forEach((raw, index) => {
    const line = index + 1;
    const s = raw;
    let i = skipSpace(s, 0);
    if (i >= s.length || s[i] === "#") return;
    if (s[i] === "[") {
      const array = s[i + 1] === "[";
      i = skipSpace(s, i + (array ? 2 : 1));
      const [name, next] = readKey(s, i, line);
      i = skipSpace(s, next);
      if (s[i] === ".") throw unsupported("dotted table name", line);
      const close = array ? "]]" : "]";
      if (!s.startsWith(close, i))
        throw unsupported("invalid table header", line);
      endOfLine(s, i + close.length, line);
      if (array) {
        const existing = Object.hasOwn(root, name) ? root[name] : undefined;
        if (
          existing !== undefined &&
          !(Array.isArray(existing) && tables.has(`[[${name}]]`))
        )
          throw unsupported("redefined key", line);
        const table: TomlTable = {};
        put(root, name, [...((existing as TomlValue[]) ?? []), table]);
        tables.add(`[[${name}]]`);
        current = table;
      } else {
        if (Object.hasOwn(root, name))
          throw unsupported("redefined table", line);
        const table: TomlTable = {};
        put(root, name, table);
        current = table;
      }
      return;
    }
    const [key, next] = readKey(s, i, line);
    i = skipSpace(s, next);
    if (s[i] === ".") throw unsupported("dotted key", line);
    if (s[i] !== "=") throw unsupported("expected '='", line);
    i = skipSpace(s, i + 1);
    const [value, end] = readValue(s, i, line);
    endOfLine(s, end, line);
    if (Object.hasOwn(current, key)) throw unsupported("duplicate key", line);
    put(current, key, value);
  });
  return root;
}
