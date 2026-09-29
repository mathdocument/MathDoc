<script lang="ts">
  // The LeanGround proof environment of this branch (plan §5, contract proof-environment.v1).
  // It is stored by the coordinator, never inside the legacy Lake project. Changing it
  // applies to new proof requests only; existing requests keep their environment.
  import { onDestroy, onMount } from "svelte";
  import { coordination, currentBranch, type Environment, type Trust } from "../lib/coordination";
  import { errMsg } from "../lib/format";
  import { ApiError } from "../lib/api";
  import { confirmDiscardDraft, removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  let { onClose }: { onClose: () => void } = $props();
  const FIELDS = [
    ["opens", "open", "Nat"],
    ["local_options", "set_option lines", "set_option maxHeartbeats 400000"],
    ["universes", "universe", "u v"],
    ["variables", "variable", "{α : Type u}"],
  ] as const;
  const branch = currentBranch();
  let baseKey = $state(""), trust = $state<Trust>("audited"), options = $state("{}");
  let context = $state<Record<string, string>>({});
  let current = $state<Environment | null>(null);
  let busy = $state(true), error = $state<string | null>(null), baseline = $state("");
  let alive = true;
  const draft = Symbol("proof environment");
  const snapshot = () => JSON.stringify([baseKey, trust, options, context]);
  $effect(() => setDraftDirty(draft, baseline !== "" && snapshot() !== baseline));
  onDestroy(() => { alive = false; removeDraft(draft); });
  function load(env: Environment | null) {
    current = env;
    baseKey = env ? String(env.base_key) : "";
    trust = env?.minimum_trust ?? "audited";
    options = JSON.stringify(env?.options ?? {});
    context = Object.fromEntries(FIELDS.map(([key]) => [key, (env?.context[key] ?? []).join("\n")]));
    baseline = snapshot();
  }
  onMount(async () => {
    try {
      if (!branch) throw new Error("open a branch first");
      load(await coordination.environment(branch.database, branch.branch));
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) load(null);
      else if (alive) error = errMsg(e);
    } finally { if (alive) busy = false; }
  });
  export function canClose() { return !busy && confirmDiscardDraft(draft); }
  async function save() {
    if (!branch || busy) return;
    busy = true; error = null; const release = trackMutation();
    try {
      const parsed = JSON.parse(options || "{}");
      const ctx: Record<string, string[]> = {};
      for (const [key] of FIELDS) {
        const lines = context[key].split("\n").map((l) => l.trim()).filter(Boolean);
        if (lines.length) ctx[key] = lines;
      }
      const saved = await coordination.putEnvironment(branch.database, branch.branch, {
        base_key: Number(baseKey), context: ctx, options: parsed, minimum_trust: trust,
      });
      if (!alive) return;
      load(saved); removeDraft(draft); busy = false;
      onClose();
    } catch (e) { if (alive) error = e instanceof SyntaxError ? "options must be a JSON object" : errMsg(e); }
    finally { busy = false; release(); }
  }
</script>
<div class="body">
  <p>Lean blocks are checked by LeanGround in this environment. Changing it affects new proof requests only.</p>
  <label>Base key<input inputmode="numeric" bind:value={baseKey} disabled={busy} placeholder="1" /></label>
  {#if current}
    <span class="meta">{current.base.root_module} · Lean {current.base.lean_version} · environment {current.environment_id.slice(0, 12)}</span>
  {/if}
  {#each FIELDS as [key, label, example]}
    <label>{label} <span class="hint">one per line, e.g. {example}</span>
      <textarea rows="2" bind:value={context[key]} disabled={busy}></textarea>
    </label>
  {/each}
  <label>Options <span class="hint">JSON object of Lean options</span><input bind:value={options} disabled={busy} /></label>
  <label>Minimum trust
    <select bind:value={trust} disabled={busy}>
      <option value="claimed">claimed</option><option value="audited">audited</option><option value="trusted">trusted</option>
    </select>
  </label>
  <p>Instances and classes cannot be registered as definitions; they must already be in this base.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
<footer class="dialog-footer actions">
  <button onclick={onClose} disabled={busy}>Cancel</button>
  <button onclick={() => void save()} disabled={busy || !/^\d+$/.test(baseKey)}>{busy ? "Saving…" : "Save proof environment"}</button>
</footer>
<style>
  .body { padding:1rem; }
  p { color:var(--mdc-muted); font-size:.85rem; }
  label { display:block; margin:.9rem 0 .3rem; font-size:.8rem; }
  .hint { color:var(--mdc-muted); font-weight:400; }
  input, textarea, select { display:block; width:100%; margin-top:.4rem; padding:.45rem; font-family:var(--mdc-mono); font-size:.8rem; color:var(--mdc-fg); background:var(--mdc-bg); border:1px solid var(--mdc-border); border-radius:4px; }
  .meta { font-family:var(--mdc-mono); font-size:.75rem; color:var(--mdc-muted); }
  .error { color:var(--mdc-error); }
</style>
