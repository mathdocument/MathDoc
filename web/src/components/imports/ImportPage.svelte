<script lang="ts">
  // The import page (docs/external-proofs.md §3.3), at /p/DATABASE/BRANCH/import: what was imported
  // from which upstream commit, the project checks LeanGround ran on it with Comparator, and the
  // proof's source — the statement module and the proof's import closure, read at that commit.
  import { onDestroy, onMount } from "svelte";
  import { ArrowLeft, ExternalLink, FileCode2, Moon, Play, Search, ShieldAlert, Sun } from "@lucide/svelte";
  import Account from "../Account.svelte";
  import LeanViewer from "./LeanViewer.svelte";
  import {
    importsApi,
    inProgress,
    verdict,
    type CheckTarget,
    type ImportCheck,
    type ImportRecord,
    type SourceFile,
    type SourceModule,
    type SourceTree,
  } from "../../lib/imports";
  import { ApiError } from "../../lib/api";
  import { projectName, projectPath } from "../../lib/project-path";
  import { applyTheme, currentTheme, observeTheme, type Theme } from "../../lib/theme";
  import "./imports.css";
  let { onReady }: { onReady?: () => void } = $props();

  const project = projectName() ?? "";
  const database = project.split("/")[0];
  const params = new URLSearchParams(location.search);
  let tab = $state<"check" | "source">(params.get("tab") === "source" ? "source" : "check");
  let selected = $state<string | null>(params.get("module"));
  let theme = $state<Theme>(currentTheme());

  let record = $state<ImportRecord | null>(null);
  let checks = $state<ImportCheck[]>([]);
  let loadError = $state<string | null>(null);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);
  let tree = $state<SourceTree | null>(null);
  let treeError = $state<string | null>(null);
  let file = $state<SourceFile | null>(null);
  let fileError = $state<string | null>(null);
  let statement = $state<SourceFile | null>(null);
  let filter = $state("");

  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const short = (s: string | null | undefined, n = 12) => (s ? s.slice(0, n) : "—");
  const blob = (path: string) => (record?.url ? `${record.url.replace(/\/$/, "")}/blob/${record.commit}/${path}` : null);
  const latest = $derived(checks[0]);
  const v = $derived(verdict(latest));
  const result = $derived(latest?.result);
  const theorems = $derived((result?.targets ?? []).filter((t) => t.kind === "theorem"));
  const holes = $derived((result?.targets ?? []).filter((t) => t.kind === "definition"));
  const commitMismatch = $derived(Boolean(result?.identity?.commit && record && result.identity.commit !== record.commit));

  async function load() {
    try {
      const r = await importsApi.get(database);
      record = r.import;
      checks = r.checks;
      loadError = null;
    } catch (e) {
      loadError = e instanceof ApiError && e.status === 404 ? "This database has no import record." : message(e);
    }
  }
  async function runCheck() {
    submitting = true;
    submitError = null;
    try {
      await importsApi.check(database);
      await load();
    } catch (e) {
      submitError = e instanceof ApiError && e.status === 403 ? "Only the database's owners can submit checks." : message(e);
    } finally {
      submitting = false;
    }
  }
  // While a check is in LeanGround, ask again every ten seconds.
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const waiting = latest && inProgress(latest.status);
    clearTimeout(timer);
    if (waiting) timer = setTimeout(() => void load(), 10000);
  });

  async function loadTree() {
    if (tree || treeError) return;
    try {
      tree = await importsApi.source(database);
    } catch (e) {
      treeError = message(e);
    }
  }
  async function loadStatement() {
    if (!record || statement) return;
    try {
      statement = await importsApi.file(database, record.statement.module);
    } catch {
      /* the statement card falls back to the module name */
    }
  }
  $effect(() => {
    if (record) void loadStatement();
  });
  $effect(() => {
    if (tab === "source") void loadTree();
  });
  // The selected module: the proof's root until one is chosen.
  const current = $derived(selected ?? tree?.root ?? null);
  $effect(() => {
    const m = current;
    if (tab !== "source" || !m || file?.module === m) return;
    fileError = null;
    importsApi
      .file(database, m)
      .then((f) => {
        if (current === f.module) file = f;
      })
      .catch((e) => (fileError = message(e)));
  });
  // The tab and module are in the address, so a link opens the same place.
  $effect(() => {
    const q = new URLSearchParams();
    if (tab === "source") q.set("tab", "source");
    if (tab === "source" && selected) q.set("module", selected);
    const next = `${location.pathname}${q.size ? `?${q}` : ""}`;
    if (next !== `${location.pathname}${location.search}`) history.replaceState(null, "", next);
  });

  const byModule = $derived(new Map((tree?.modules ?? []).map((m) => [m.module, m])));
  const importedBy = $derived.by(() => {
    const out = new Map<string, string[]>();
    for (const m of tree?.modules ?? []) for (const i of m.imports) out.set(i, [...(out.get(i) ?? []), m.module]);
    return out;
  });
  // Modules grouped by directory, filtered by name.
  const groups = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const out = new Map<string, SourceModule[]>();
    for (const m of tree?.modules ?? []) {
      if (q && !m.module.toLowerCase().includes(q)) continue;
      const dir = m.module.split(".").slice(0, -1).join(".");
      out.set(dir, [...(out.get(dir) ?? []), m]);
    }
    return [...out.entries()];
  });
  const leaf = (m: string) => m.split(".").at(-1);
  // The scope note's title often repeats the result's name, up to quotes and case.
  const fold = (s: string) => s.toLowerCase().replace(/[’‘`]/g, "'").replace(/\s+/g, " ").trim();
  const same = (a: string, b: string | null) => (b === null ? true : fold(a) === fold(b));
  // The chosen module stays in view in the list.
  let files = $state<HTMLElement>();
  $effect(() => {
    const m = current;
    if (!files || !m || !tree) return;
    requestAnimationFrame(() => files?.querySelector<HTMLElement>(`[data-module="${CSS.escape(m)}"]`)?.scrollIntoView({ block: "nearest" }));
  });

  function select(m: string) {
    selected = m;
    tab = "source";
  }
  function toggleTheme() {
    theme = theme === "dark" ? "light" : "dark";
    applyTheme(theme);
  }
  const holeText = (t: CheckTarget) => {
    if (t.challenge === "sorry") return "Left to the proof (sorry in the statement)";
    if (t.comparison === "identical") return "Same as the statement";
    if (t.comparison === "differs") return "Differs from the statement";
    if (t.comparison === "unchecked") return "Not compared";
    return "—";
  };

  onMount(() => {
    void load().finally(() => onReady?.());
    return observeTheme((next) => {
      theme = next;
      applyTheme(next, false);
    });
  });
  onDestroy(() => clearTimeout(timer));
</script>

<div class="import-page">
  <header class="app-header">
    <a class="app-brand" href="/" aria-label="All projects" title="All projects"><img src="/mdc-logo.svg" alt="" /><strong>MathDoc</strong></a>
    <a class="crumb" href={projectPath("/")} title="Back to the document"><ArrowLeft size={15} strokeWidth={1.9} />{project}</a>
    <span class="crumb-sep">/</span>
    <span class="crumb-here">Import</span>
    <nav class="tabs" aria-label="Import sections">
      <button aria-current={tab === "check" ? "page" : undefined} onclick={() => (tab = "check")}>Check</button>
      <button aria-current={tab === "source" ? "page" : undefined} onclick={() => (tab = "source")}>Source</button>
    </nav>
    <span style="flex: 1"></span>
    <button class="header-theme" onclick={toggleTheme} title="Toggle theme" aria-label="Toggle theme">
      {#if theme === "dark"}<Sun size={16} strokeWidth={1.8} />{:else}<Moon size={16} strokeWidth={1.8} />{/if}
    </button>
    <Account />
  </header>

  {#if loadError}
    <main class="import-main"><p class="error">{loadError}</p></main>
  {:else if !record}
    <main class="import-main"><p class="muted">Loading…</p></main>
  {:else if tab === "check"}
    <main class="import-main scroll" aria-label="Import and checks">
      <section class="hero">
        <p class="eyebrow">
          Imported from {#if record.url}<a href={`${record.url}/tree/${record.commit}`} target="_blank" rel="noreferrer">{record.url.replace(/^https?:\/\//, "")}</a>{:else}{record.source}{/if}
          at <code>{short(record.commit, 9)}</code> · Comparator configuration
          {#if blob(`${record.statement.path.split("/")[0]}/ComparatorChallenges/${record.config_name}.json`)}<a href={blob(`${record.statement.path.split("/")[0]}/ComparatorChallenges/${record.config_name}.json`)} target="_blank" rel="noreferrer"><code>{record.config_name}</code></a>{:else}<code>{record.config_name}</code>{/if}
          · scope note {record.scope.family}
        </p>
        <h1>{record.scope.result ?? record.scope.title} <span class="verdict" data-tone={v.tone}>{v.label}</span></h1>
        {#if same(record.scope.title, record.scope.result) === false}<p class="sub">{record.scope.title}</p>{/if}
      </section>

      <div class="cards">
        <section class="card wide" aria-label="Project check">
          <div class="card-head">
            <h2>Project check</h2>
            <button class="primary" onclick={runCheck} disabled={submitting || (latest && inProgress(latest.status))}>
              <Play size={14} strokeWidth={2} />{latest ? "Check again" : "Run check"}
            </button>
          </div>
          {#if submitError}<p class="error">{submitError}</p>{/if}
          {#if !latest}
            <p class="muted">LeanGround has not checked this import yet. A check builds the statement and the proof in the pinned workspace and compares them with Comparator.</p>
          {:else}
            <p class="meta">
              Submitted by {latest.submitted_by} at {new Date(latest.submitted_at).toLocaleString()}
              {#if latest.completed_at}· finished {new Date(latest.completed_at).toLocaleString()}{/if}
              {#if latest.leanground_check}· LeanGround <code>{latest.leanground_check}</code>{/if}
            </p>
            {#if inProgress(latest.status)}
              <p class="muted">LeanGround is {latest.status === "running" ? "checking it" : "about to check it"}; this page asks again every ten seconds. A first check builds the proof and can take a long time.</p>
            {/if}
            {#if latest.status === "submit_failed"}<p class="error">The check could not be submitted ({latest.error}). Submitting again resends it; LeanGround runs it once.</p>{/if}
            {#if result?.isolated === false}
              <p class="warn"><ShieldAlert size={15} strokeWidth={1.9} />Not isolated: Comparator ran without a sandbox, so this result assumes the upstream code does nothing harmful while it compiles. It is not a formal verification record.</p>
            {/if}
            {#if commitMismatch}<p class="warn">LeanGround checked commit <code>{short(result?.identity?.commit, 9)}</code>, not the imported <code>{short(record.commit, 9)}</code>.</p>{/if}
            {#if result?.reason && latest.status !== "passed"}
              <p class="error"><strong>{result.reason.replaceAll("_", " ")}</strong>{#if result.message}: {result.message}{/if}</p>
            {/if}
            {#if result?.targets?.length}
              <table class="targets">
                <thead><tr><th>Target</th><th>Result</th><th>Definition</th></tr></thead>
                <tbody>
                  {#each [...theorems, ...holes] as t (t.name)}
                    <tr>
                      <td><code>{t.name}</code><span class="kind">{t.kind === "theorem" ? "theorem" : "definition hole"}</span></td>
                      <td><span class="pill" data-status={t.status}>{t.status}</span></td>
                      <td>
                        {#if t.kind === "definition"}
                          <span class:flag={t.needs_review}>{holeText(t)}</span>{#if t.needs_review}<span class="pill review">review</span>{/if}
                        {:else}—{/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            {/if}
            {#if result?.passes?.length}
              <ol class="passes">
                {#each result.passes as p, i (i)}
                  <li>Pass {i + 1}{i === 1 ? " (definition holes with a body compared)" : ""}: <span class="pill" data-status={p.status}>{p.status}</span>{#if p.reason} {p.reason.replaceAll("_", " ")}{/if} · {Math.round(p.seconds)} s</li>
                {/each}
              </ol>
            {/if}
            {#if result?.identity}
              <dl class="identity">
                <dt>Workspace</dt><dd>{result.workspace} · commit <code>{short(result.identity.commit, 9)}</code>{result.identity.dirty ? " (with local changes)" : ""}</dd>
                <dt>Toolchain</dt><dd><code>{result.identity.lean_toolchain ?? "—"}</code></dd>
                <dt>Lake manifest</dt><dd><code>{short(result.identity.lake_manifest_sha256)}</code></dd>
                <dt>Statement · proof</dt><dd><code>{short(result.identity.challenge_sha256)}</code> · <code>{short(result.identity.solution_sha256)}</code></dd>
              </dl>
            {/if}
            {#if result?.log_tail}
              <details class="log"><summary>Comparator output (last lines)</summary><pre>{result.log_tail}</pre></details>
            {/if}
          {/if}
          {#if checks.length > 1}
            <details class="history">
              <summary>Earlier checks ({checks.length - 1})</summary>
              <ul>
                {#each checks.slice(1) as c (c.id)}
                  <li><span class="verdict small" data-tone={verdict(c).tone}>{verdict(c).label}</span> {new Date(c.submitted_at).toLocaleString()} · {c.submitted_by}</li>
                {/each}
              </ul>
            </details>
          {/if}
        </section>

        <section class="card" aria-label="What is checked">
          <h2>What is checked</h2>
          <dl class="facts">
            <dt>Statement</dt><dd><button class="link" onclick={() => select(record!.statement.module)}><code>{record.statement.module}</code></button></dd>
            <dt>Proof</dt><dd><button class="link" onclick={() => select(record!.proof.module)}><code>{record.proof.module}</code></button><br /><span class="muted">{record.proof.files} files, {record.proof.lines.toLocaleString()} lines; also imports {record.proof.external.join(", ") || "nothing else"}</span></dd>
            <dt>Theorems</dt><dd>{#each record.config.theorem_names as n (n)}<code class="block">{n}</code>{/each}</dd>
            <dt>Definition holes</dt><dd>{#if record.config.definition_names.length}{#each record.config.definition_names as n (n)}<code class="block">{n}</code>{/each}{:else}none{/if}</dd>
            <dt>Permitted axioms</dt><dd>{record.config.permitted_axioms.join(", ")}</dd>
          </dl>
          <p class="muted small">A passing check means: the proof states these theorems exactly as the statement module does, the declarations they use are the same (definition holes only by type), the kernel replays their dependencies, and they use only the permitted axioms. It does not judge whether the statement says what the paper claims.</p>
        </section>

        <section class="card" aria-label="Sources">
          <h2>Paper and scope</h2>
          <ul class="links">
            {#each record.papers as p (p.path)}
              <li>{#if blob(p.path)}<a href={blob(p.path)} target="_blank" rel="noreferrer">{p.title}<ExternalLink size={12} /></a>{:else}{p.title}{/if}{#if !p.present}<span class="muted"> (not in the repository)</span>{/if}</li>
            {/each}
            <li>{#if blob(record.scope.path)}<a href={blob(record.scope.path)} target="_blank" rel="noreferrer">Scope note {record.scope.family}<ExternalLink size={12} /></a>{:else}Scope note {record.scope.family}{/if}</li>
            <li><a href={projectPath(`/#ref=${record.nodes.overview}`)}>The imported document</a></li>
          </ul>
        </section>

        <section class="card wide statement" aria-label="Statement">
          <div class="card-head">
            <h2>Statement <code>{record.statement.module}</code></h2>
            <button class="link" onclick={() => select(record!.statement.module)}><FileCode2 size={14} />Open in Source</button>
          </div>
          {#if statement}<div class="viewer short"><LeanViewer text={statement.text} name={statement.module} /></div>{:else}<p class="muted">Reading the statement…</p>{/if}
        </section>
      </div>
    </main>
  {:else}
    <main class="import-main source" aria-label="Source">
      <aside class="files" aria-label="Modules" bind:this={files}>
        <label class="filter"><Search size={14} /><input placeholder="Filter modules" bind:value={filter} /></label>
        {#if treeError}<p class="error">{treeError}</p>{:else if !tree}<p class="muted">Reading the import closure…</p>{:else}
          <p class="muted small">Statement and the proof's {tree.modules.length} modules at <code>{short(tree.commit, 9)}</code>; outside the repository: {tree.external.join(", ")}.</p>
          <button class="file" class:on={current === tree.statement.module} data-module={tree.statement.module} onclick={() => select(tree!.statement.module)}>
            <span class="name">Statement</span><span class="lines">{tree.statement.lines}</span>
          </button>
          {#each groups as [dir, mods] (dir)}
            <details open={groups.length < 12 || mods.some((m) => m.module === current)}>
              <summary title={dir}>{dir.split(".").slice(-2).join(".")}<span class="lines">{mods.length}</span></summary>
              {#each mods as m (m.module)}
                <button class="file" class:on={current === m.module} class:root={m.module === tree.root} data-module={m.module} onclick={() => select(m.module)} title={m.module}>
                  <span class="name">{leaf(m.module)}</span><span class="lines">{m.lines}</span>
                </button>
              {/each}
            </details>
          {/each}
        {/if}
      </aside>
      <section class="file-view" aria-label="File">
        {#if fileError}<p class="error">{fileError}</p>{:else if !file}<p class="muted">Reading…</p>{:else}
          <div class="file-head">
            <code class="module">{file.module}</code>
            {#if file.url}<a href={file.url} target="_blank" rel="noreferrer" title="On the upstream repository">{file.path}<ExternalLink size={12} /></a>{:else}<span class="muted">{file.path}</span>{/if}
          </div>
          {#if byModule.get(file.module)?.imports.length || importedBy.get(file.module)?.length}
            <div class="relations">
              {#if byModule.get(file.module)?.imports.length}
                <span class="label">Imports</span>{#each byModule.get(file.module)!.imports as i (i)}<button class="chip" onclick={() => select(i)} title={i}>{leaf(i)}</button>{/each}
              {/if}
              {#if importedBy.get(file.module)?.length}
                <span class="label">Imported by</span>{#each importedBy.get(file.module)! as i (i)}<button class="chip" onclick={() => select(i)} title={i}>{leaf(i)}</button>{/each}
              {/if}
            </div>
          {/if}
          <div class="viewer"><LeanViewer text={file.text} name={file.module} /></div>
        {/if}
      </section>
    </main>
  {/if}
</div>
