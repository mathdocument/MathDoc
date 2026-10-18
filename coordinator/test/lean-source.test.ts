import test from "node:test";
import assert from "node:assert/strict";
import {
  body,
  classify,
  contextMismatch,
  mask,
  placeholder,
  proposition,
  scan,
  withoutPlaceholders,
} from "../src/lean-source.js";

test("masking keeps offsets and newlines, and nests block comments", () => {
  const src = 'a -- c\n/- x /- y -/ z -/b "s -- t"';
  const m = mask(src);
  assert.equal(m.length, src.length);
  assert.equal(m.split("\n").length, src.split("\n").length);
  assert.ok(!m.includes("c") && !m.includes("z") && m.includes("b"));
  assert.ok(!m.includes("t") && mask(src, true).includes('"s -- t"'));
});

test("a theorem node: prelude, statement, placeholder and proposition", () => {
  const src = [
    "import Lib.N_1",
    "open Nat",
    "set_option maxHeartbeats 400",
    "",
    "/-- doc -/",
    "theorem add_zero' (n : Nat) {m : Nat} -- note",
    "    (h : m = n) : n + 0 = m := by",
    "  simp [h]",
    "",
  ].join("\n");
  const s = scan(src);
  assert.equal(s.rejection, undefined);
  assert.deepEqual(s.imports, ["Lib.N_1"]);
  assert.deepEqual(s.context, {
    opens: ["Nat"],
    local_options: ["set_option maxHeartbeats 400"],
  });
  const [d] = s.decls;
  assert.equal(d.name, "add_zero'");
  assert.equal(d.binders, "(n : Nat) {m : Nat} (h : m = n)");
  assert.equal(d.type, "n + 0 = m");
  assert.equal(d.sorry, false);
  assert.equal(proposition(d), "∀ (n : Nat) {m : Nat} (h : m = n), n + 0 = m");
  assert.equal(
    placeholder(d),
    "theorem add_zero' (n : Nat) {m : Nat} (h : m = n) : n + 0 = m := sorry\n",
  );
  assert.ok(body(s).startsWith("theorem add_zero'"));
  assert.ok(!body(s).includes("import") && !body(s).includes("open"));
  const c = classify(s, {});
  assert.equal(c.role, "theorem");
  assert.equal(c.role === "theorem" && c.source, "syntax_scan");
});

test("module headers and public imports are imports", () => {
  const s = scan(
    "module\n\npublic import Mathlib.Data.Nat.Basic\npublic meta import Lean\ntheorem t : True := trivial\n",
  );
  assert.equal(s.rejection, undefined);
  assert.deepEqual(s.imports, ["Mathlib.Data.Nat.Basic", "Lean"]);
  assert.equal(body(s), "theorem t : True := trivial\n");
});

test("sorry bodies, definitions and placeholder stripping", () => {
  const open = scan("theorem t : 1 = 1 := by sorry\n");
  assert.equal(open.decls[0].sorry, true);
  assert.equal(
    withoutPlaceholders(
      "theorem p : True := sorry\ntheorem t : True := by\n  exact p\n",
    ),
    "theorem t : True := by\n  exact p\n",
  );
  const defs = scan(
    "def double (n : Nat) : Nat := n + n\nnoncomputable def f : Nat := 0\nstructure P where\n  x : Nat\n",
  );
  assert.deepEqual(
    defs.decls.map((d) => [d.kind, d.name]),
    [
      ["def", "double"],
      ["noncomputable_def", "f"],
      ["structure", "P"],
    ],
  );
  assert.deepEqual(classify(defs, {}), {
    role: "definition",
    source: "syntax_scan",
    names: ["double", "f", "P"],
  });
});

test("ambiguous and unsupported blocks are rejected with a reason, never guessed", () => {
  const reason = (src: string, metadata: Record<string, string> = {}) => {
    const c = classify(scan(src), metadata);
    return c.role === "unsupported" ? c.rejection.reason : c.role;
  };
  assert.equal(
    reason("theorem a : True := trivial\ntheorem b : True := trivial\n"),
    "multiple_conclusions",
  );
  assert.equal(
    reason("theorem a : True := trivial\ntheorem b : True := a\n", {
      lean_conclusion: "b",
    }),
    "theorem",
  );
  assert.equal(
    reason("def f : Nat := 1\ntheorem b : f = 1 := rfl\n"),
    "mixed_declarations",
  );
  assert.equal(
    reason("def f : Nat := 1\ntheorem b : 1 = 1 := rfl\n", {
      lean_role: "theorem",
      lean_conclusion: "b",
    }),
    "theorem",
  );
  assert.equal(
    reason("theorem b : 1 = 1 := rfl\n", { lean_role: "definition" }),
    "mixed_declarations",
  );
  assert.equal(
    reason("instance : Inhabited Nat := ⟨0⟩\n"),
    "unsupported_declaration",
  );
  assert.equal(reason("namespace X\n"), "unsupported_declaration");
  assert.equal(
    reason("@[simp] theorem a : True := trivial\n"),
    "unsupported_declaration",
  );
  assert.equal(
    reason("open Nat in\ntheorem a : True := trivial\n"),
    "unsupported_declaration",
  );
  assert.equal(reason("-- only a comment\n"), "ambiguous_binding");
  assert.equal(reason("theorem a\n  | 0 => rfl\n"), "ambiguous_binding");
  assert.equal(
    reason("theorem a : True := trivial\n", { lean_role: "x" }),
    "ambiguous_binding",
  );
});

test("context commands must equal the fixed environment exactly", () => {
  assert.equal(contextMismatch({ opens: ["Nat"] }, { opens: ["Nat"] }), null);
  assert.equal(contextMismatch({}, { opens: ["Nat"] }), null);
  assert.match(
    contextMismatch({ opens: ["Nat"] }, { opens: ["Nat", "List"] })!,
    /opens/,
  );
  assert.match(contextMismatch({ opens: ["Nat"] }, {})!, /opens/);
});
