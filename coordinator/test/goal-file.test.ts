// The goal file format mdc-goal-file/1 (docs/mathdoc.md §4.11): rendering a draft and turning
// edited files into submissions. Pure functions; no services.
import test from "node:test";
import assert from "node:assert/strict";
import { mentions, parse, render, SEPARATOR } from "../src/mdc/goal-file.js";
import { MdcError } from "../src/mdc/client.js";
import type { GoalDraft } from "../src/goal-draft.js";

const K = (c: string) => c.repeat(64);
const draft = (over: Partial<GoalDraft> = {}): GoalDraft => ({
  project: "mdc-1",
  revision: 3,
  environment_id: "env",
  goal: K("a"),
  status: "insufficient",
  branch: "main",
  root_module: "Init",
  context: {
    local_options: ["set_option autoImplicit false"],
    opens: ["Nat"],
    variables: ["{α : Type}"],
  },
  options: { maxHeartbeats: 400000, pp_all: false },
  root: {
    goal: K("a"),
    name: "double_eq",
    binders: "(n : Nat)",
    type: "double n = 2 * n",
    definitions: [K("d")],
    from: "node",
  },
  premises: [
    {
      goal: K("b"),
      name: "two_mul_eq",
      binders: "(n : Nat)",
      type: "2 * n = n + n",
      definitions: [],
      from: "node",
    },
  ],
  definitions: [
    {
      id: K("d"),
      name: "double",
      source: "def double (n : Nat) : Nat := n + n",
    },
  ],
  notes: [],
  task: null,
  ...over,
});
const rejects = (fn: () => unknown, reason: string) =>
  assert.throws(fn, (e: unknown) => {
    assert.ok(e instanceof MdcError, String(e));
    assert.equal(e.reason, reason, e.message);
    return true;
  });
/** Replace the generated proof of the conclusion. */
const prove = (text: string, proof: string) =>
  text.replace(":= by\n  sorry\n", `:= by\n  ${proof}\n`);

test("a pulled file has the checking header's order, the premises and the goal", () => {
  const r = render(draft());
  const lines = r.text.split("\n");
  assert.equal(lines[0], "-- mdc-goal-file/1");
  const at = (l: string) => {
    const i = lines.indexOf(l);
    assert.ok(i >= 0, `missing ${l}`);
    return i;
  };
  // LeanGround's order: options, universes, opens, definitions, notation, attributes, variables.
  const order = [
    "import Init",
    "set_option autoImplicit false",
    "set_option maxHeartbeats 400000",
    "set_option pp_all false",
    "open Nat",
    "def double (n : Nat) : Nat := n + n",
    "variable {α : Type}",
    SEPARATOR,
    "theorem two_mul_eq (n : Nat) : 2 * n = n + n := sorry",
    "theorem double_eq (n : Nat) : double n = 2 * n := by",
  ].map(at);
  assert.deepEqual(
    order,
    [...order].sort((a, b) => a - b),
  );
  assert.equal(lines.filter((l) => l === SEPARATOR).length, 1);
  assert.equal(r.root, "double_eq");
  assert.equal(r.root_signature, "(n : Nat) : double n = 2 * n");
  assert.deepEqual(r.premises, [{ name: "two_mul_eq", goal: K("b") }]);
  assert.deepEqual(r.definitions, [K("d")]);
});

test("names stay unique and free of quotes", () => {
  const r = render(
    draft({
      root: { ...draft().root, name: "double" },
      premises: [{ ...draft().premises[0], name: "double" }],
    }),
  );
  assert.equal(r.root, "double_goal");
  assert.deepEqual(
    r.premises.map((p) => p.name),
    ["double_premise"],
  );
  const q = render(draft({ root: { ...draft().root, name: "foo'" } }));
  assert.equal(q.root, "foo_");
});

test("a complete proof drops the unused pulled premise and keeps the rest verbatim", () => {
  const r = render(draft());
  const p = parse(prove(r.text, "unfold double\n  omega"), r);
  assert.equal(p.mode, "leaf");
  assert.equal(p.component, "lean-worker");
  assert.deepEqual(p.dropped, ["two_mul_eq"]);
  assert.ok(!p.source.includes("two_mul_eq"));
  assert.ok(p.source.includes("-- Goal. Do not change its statement."));
  assert.ok(p.source.includes("  unfold double\n  omega\n"));
  assert.ok(!p.source.includes("import"), "only the body is submitted");
  assert.equal(p.root_is_sorry, false);
  // CRLF line endings read the same.
  assert.deepEqual(
    parse(prove(r.text, "unfold double\n  omega").replaceAll("\n", "\r\n"), r),
    p,
  );
});

test("a decomposition keeps the premises it uses and adds new ones", () => {
  const r = render(draft());
  const body = prove(
    r.text,
    "induction n with\n  | zero => rfl\n  | succ k ih => rw [double_succ, ih]; omega",
  ).replace(
    "\n-- Goal.",
    "\ntheorem double_succ (n : Nat) : double (n + 1) = double n + 2 := by sorry\n\n-- Goal.",
  );
  const p = parse(body, r);
  assert.equal(p.mode, "sketch");
  assert.equal(p.component, "lean-worker-sketch");
  assert.deepEqual(p.placeholders, ["double_succ"]);
  assert.deepEqual(p.dropped, ["two_mul_eq"]);
  // Referencing the pulled premise keeps it; so does a mention as `h.name` or `name.symm`.
  const uses = parse(prove(r.text, "exact (two_mul_eq n).symm ▸ rfl"), r);
  assert.deepEqual(uses.placeholders, ["two_mul_eq"]);
  assert.deepEqual(uses.dropped, []);
});

test("definitions a premise statement mentions are registered with the submission", () => {
  const r = render(draft());
  const body = prove(r.text, "rw [triple_eq]; omega").replace(
    "\n-- Goal.",
    "\ndef base (n : Nat) : Nat := n\n" +
      "def triple (n : Nat) : Nat := base n + n + n\n" +
      "def local_helper (n : Nat) : Nat := n\n" +
      "theorem triple_eq (n : Nat) : double n = triple n - n := sorry\n\n-- Goal.",
  );
  const p = parse(body, r);
  assert.deepEqual(p.new_definitions, ["base", "triple"]);
});

test("files that break the rules are refused with the documented reasons", () => {
  const r = render(draft());
  const ok = prove(r.text, "rfl");
  const before = (text: string, insert: string) =>
    text.replace("\n-- Goal.", `\n${insert}\n-- Goal.`);
  rejects(
    () => parse(ok.replace("-- mdc-goal-file/1", "-- mdc-goal-file/2"), r),
    "unsupported_format",
  );
  rejects(
    () => parse(ok.replace(`${SEPARATOR}\n`, ""), r),
    "separator_missing",
  );
  rejects(() => parse(before(ok, SEPARATOR), r), "separator_duplicated");
  rejects(
    () => parse(ok.replace("open Nat", "open Nat Int"), r),
    "prefix_modified",
  );
  rejects(() => parse(before(ok, "open Int"), r), "context_in_body");
  rejects(
    () => parse(before(ok, "example : True := trivial"), r),
    "unsupported_declaration",
  );
  rejects(
    () => parse(before(ok, "@[simp] theorem s : True := trivial"), r),
    "unsupported_declaration",
  );
  rejects(
    () => parse(before(ok, "private theorem s : True := trivial"), r),
    "unsupported_declaration",
  );
  rejects(
    () => parse(ok.replace("theorem double_eq", "theorem double_eq2"), r),
    "root_missing",
  );
  rejects(
    () =>
      parse(
        before(ok, "theorem double_eq (n : Nat) : double n = 2 * n := rfl"),
        r,
      ),
    "root_duplicated",
  );
  rejects(
    () =>
      parse(ok.replace("double n = 2 * n := by", "double n = n * 2 := by"), r),
    "root_statement_changed",
  );
  rejects(
    () => parse(before(ok, "theorem double : True := trivial"), r),
    "duplicate_name",
  );
  rejects(
    () => parse(before(ok, "theorem h' : True := trivial"), r),
    "invalid_name",
  );
  rejects(
    () =>
      parse(
        before(ok, "theorem again (n : Nat) : double n = 2 * n := sorry"),
        r,
      ),
    "premise_equals_root",
  );
  // An unproved goal parses (for drafts) and says so; mdc prove refuses it.
  assert.equal(parse(r.text, r).root_is_sorry, true);
});

test("identifier mentions ignore longer names and comments", () => {
  assert.ok(mentions("rw [two_mul_eq]", "two_mul_eq"));
  assert.ok(mentions("exact h.two_mul_eq", "two_mul_eq"));
  assert.ok(mentions("(two_mul_eq n).symm", "two_mul_eq"));
  assert.ok(mentions("two_mul_eq.symm", "two_mul_eq"));
  assert.ok(!mentions("two_mul_eq_x", "two_mul_eq"));
  assert.ok(!mentions("my_two_mul_eq", "two_mul_eq"));
  assert.ok(!mentions("two_mul_eq'", "two_mul_eq"));
  assert.ok(mentions("Foo.bar x", "Foo.bar"));
  assert.ok(!mentions("FooXbar", "Foo.bar"));
});
