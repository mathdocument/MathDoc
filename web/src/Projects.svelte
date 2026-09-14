<script lang="ts">
  import { onMount } from "svelte";
  import { ArrowUpRight, FolderOpen, GitBranch, Moon, RefreshCw, Search, Sun } from "@lucide/svelte";
  import { fetchJson, isAbortError, type ServiceStatus } from "./lib/api";
  import { applyTheme, currentTheme, observeTheme, type Theme } from "./lib/theme";

  let status = $state<ServiceStatus | null>(null);
  let error = $state<string | null>(null);
  let refreshing = $state(false);
  let updated = $state("");
  let query = $state("");
  let filter = $state<"all" | "running" | "stopped">("all");
  let theme = $state<Theme>(currentTheme());
  const controller = new AbortController();
  const branches = $derived(Object.entries(status?.projects ?? {}).map(([name, state]) => {
    const [database, branch] = name.split("/");
    return { name, database: database!, branch: branch!, ...state };
  }));
  const running = $derived(branches.filter(b => b.running).length);
  const projectCount = $derived(new Set(branches.map(b => b.database)).size);
  const groups = $derived.by(() => {
    const filtered = branches.filter(b => b.name.toLowerCase().includes(query.trim().toLowerCase()) &&
      (filter === "all" || b.running === (filter === "running")));
    const groups = new Map<string, typeof branches>();
    for (const branch of filtered) {
      const rows = groups.get(branch.database) ?? [];
      rows.push(branch); groups.set(branch.database, rows);
    }
    return [...groups.entries()];
  });

  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    try {
      status = await fetchJson<ServiceStatus>("/api/status", {
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]),
      });
      updated = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      error = null;
    } catch (e) {
      if (!isAbortError(e)) error = e instanceof Error ? e.message : String(e);
    } finally { refreshing = false; }
  }
  function toggleTheme() {
    theme = theme === "dark" ? "light" : "dark";
    applyTheme(theme);
  }
  onMount(() => {
    document.title = "Projects · MathDoc";
    void refresh();
    const check = () => { if (!document.hidden) void refresh(); };
    const timer = setInterval(check, 5000);
    document.addEventListener("visibilitychange", check);
    window.addEventListener("focus", check);
    const stopTheme = observeTheme(next => { theme = next; applyTheme(next, false); });
    return () => {
      clearInterval(timer); controller.abort(); stopTheme();
      document.removeEventListener("visibilitychange", check);
      window.removeEventListener("focus", check);
    };
  });
</script>

<div class="directory">
  <header class="topbar">
    <a class="brand" href="/" aria-label="MathDoc projects"><img src="/mdc-logo.svg" alt="" /><strong>MathDoc</strong></a>
    <span class="section-label">Projects</span>
    <button class="icon-button theme-button" onclick={toggleTheme} title="Toggle theme" aria-label="Toggle theme">
      {#if theme === "dark"}<Sun size={17} />{:else}<Moon size={17} />{/if}
    </button>
  </header>

  <main>
    <div class="intro">
      <div><p class="eyebrow">MATHEMATICAL WORKSPACE</p><h1>Projects</h1><p class="subtitle">Open a running branch to explore and edit its graph.</p></div>
      <div class="overview" aria-label="Project summary">
        <span><strong>{projectCount}</strong> projects</span><span><strong>{branches.length}</strong> branches</span>
        <span class="running-total"><i></i><strong>{running}</strong> running</span>
      </div>
    </div>

    <div class="controls">
      <label class="search"><Search size={17} strokeWidth={1.8} /><input aria-label="Search projects and branches" placeholder="Search projects or branches…" bind:value={query} /></label>
      <div class="filters" aria-label="Filter branches">
        <button aria-pressed={filter === "all"} onclick={() => filter = "all"}>All branches</button>
        <button aria-pressed={filter === "running"} onclick={() => filter = "running"}>Running</button>
        <button aria-pressed={filter === "stopped"} onclick={() => filter = "stopped"}>Stopped</button>
      </div>
      <button class="icon-button" class:refreshing onclick={() => void refresh()} disabled={refreshing} title="Refresh projects" aria-label="Refresh projects"><RefreshCw size={16} /></button>
    </div>

    {#if error}<div class="error" role="alert">Could not refresh projects: {error}{#if status}<span>Showing the last known states.</span>{/if}</div>{/if}
    {#if !status && !error}
      <div class="empty" role="status">Loading projects…</div>
    {:else if status && branches.length === 0}
      <div class="empty"><FolderOpen size={30} /><h2>No projects yet</h2><p>Create a project with <code>mdc init NAME</code>, then start its main branch.</p></div>
    {:else if status && groups.length === 0}
      <div class="empty"><Search size={28} /><h2>No matching branches</h2><p>Try a different name or change the status filter.</p></div>
    {:else}
      <div class="projects">
        {#each groups as [database, rows] (database)}
          <section class="project" aria-label={`Project ${database}`}>
            <div class="project-head"><FolderOpen size={18} strokeWidth={1.7} /><h2>{database}</h2><span>{rows.length} {rows.length === 1 ? "branch" : "branches"}</span></div>
            {#each rows as row (row.name)}
              <div class="branch-row" data-project={row.name}>
                <span class="branch-name"><GitBranch size={16} strokeWidth={1.6} /><span>{row.branch}</span>{#if row.branch === "main"}<small>MAIN</small>{/if}</span>
                <span class="state" class:online={row.running}><i></i>{row.running ? "Running" : "Stopped"}</span>
                {#if row.running && row.url}
                  <a class="open" href={`/p/${row.name}/`} aria-label={`Open ${row.name}`}>Open<ArrowUpRight size={16} /></a>
                {:else}<span class="unavailable" title="This branch has no active service">—</span>{/if}
              </div>
            {/each}
          </section>
        {/each}
      </div>
    {/if}
    {#if updated}<footer>Last refreshed {updated}<span>Updates automatically</span></footer>{/if}
  </main>
</div>

<style>
  .directory { height: 100%; overflow: auto; }
  .topbar { display: flex; align-items: center; gap: 22px; height: 58px; padding: 0 28px; border-bottom: 1px solid var(--mdc-border); background: color-mix(in srgb, var(--mdc-panel) 82%, transparent); }
  .brand { display: flex; align-items: center; gap: 9px; color: var(--mdc-fg); text-decoration: none; letter-spacing: -.03em; }
  .brand img { width: 27px; height: 27px; }
  .section-label { color: var(--mdc-dim); padding-left: 22px; border-left: 1px solid var(--mdc-border-strong); font-size: 12px; }
  .theme-button { margin-left: auto; }
  main { max-width: 1144px; padding: 58px 32px 28px; margin: auto; }
  .intro { display: flex; align-items: flex-end; justify-content: space-between; gap: 24px; margin-bottom: 32px; }
  .eyebrow { color: var(--mdc-accent); font-size: 10px; letter-spacing: .13em; font-weight: 650; margin: 0 0 12px; }
  h1 { font-size: 34px; line-height: 1.15; font-weight: 630; letter-spacing: -.035em; margin: 0; }
  .subtitle { color: var(--mdc-dim); margin: 12px 0 0; font-size: 13px; }
  .overview { display: flex; align-items: center; gap: 19px; color: var(--mdc-dim); font-size: 12px; white-space: nowrap; padding-bottom: 2px; }
  .overview strong { font-size: 14px; font-weight: 550; color: var(--mdc-fg-soft); margin-right: 3px; }
  .running-total { display: flex; align-items: center; }
  i { display: inline-block; width: 6px; height: 6px; border-radius: 50%; background: var(--mdc-muted); margin-right: 8px; }
  .running-total i, .online i { background: var(--mdc-accent-down); box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-accent-down) 9%, transparent); }
  .controls { display: flex; align-items: center; gap: 12px; margin-bottom: 24px; }
  .search { display: flex; align-items: center; gap: 10px; max-width: 430px; min-width: 170px; flex: 1; height: 40px; padding: 0 13px; border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); background: var(--mdc-panel); color: var(--mdc-dim); }
  .search:focus-within { border-color: var(--mdc-accent); }
  .search input { width: 100%; border: 0; outline: none; background: transparent; color: var(--mdc-fg); font-size: 12px; }
  .search input::placeholder { color: var(--mdc-muted); }
  .filters { display: flex; gap: 3px; padding: 3px; border-radius: var(--mdc-radius-sm); background: var(--mdc-panel); border: 1px solid var(--mdc-border); margin-left: auto; }
  button { cursor: pointer; }
  .filters button { color: var(--mdc-dim); background: transparent; border: 0; border-radius: 5px; padding: 6px 11px; font-size: 11px; white-space: nowrap; }
  .filters button[aria-pressed="true"] { color: var(--mdc-fg); background: var(--mdc-card-selected); box-shadow: var(--mdc-shadow-sm); }
  .icon-button { display: flex; align-items: center; justify-content: center; flex-shrink: 0; width: 34px; height: 34px; color: var(--mdc-dim); background: transparent; border: 1px solid transparent; border-radius: var(--mdc-radius-sm); }
  .icon-button:hover, .filters button:hover { color: var(--mdc-fg); background: var(--mdc-card-hover); }
  button:disabled { cursor: default; opacity: .5; }
  .projects { display: grid; gap: 18px; }
  .project { border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-md); background: var(--mdc-panel); overflow: hidden; }
  .project-head { display: flex; align-items: center; gap: 11px; padding: 16px 20px; color: var(--mdc-accent); background: color-mix(in srgb, var(--mdc-panel-raised) 65%, transparent); border-bottom: 1px solid var(--mdc-border); }
  h2 { color: var(--mdc-fg); font-size: 14px; font-weight: 580; letter-spacing: -.015em; margin: 0; overflow-wrap: anywhere; }
  .project-head span { color: var(--mdc-muted); font-size: 11px; margin-left: auto; white-space: nowrap; }
  .branch-row { display: grid; grid-template-columns: minmax(0, 1fr) 110px 75px; gap: 18px; align-items: center; min-height: 62px; padding: 12px 20px; }
  .branch-row + .branch-row { border-top: 1px solid var(--mdc-border); }
  .branch-name { display: flex; align-items: center; gap: 12px; min-width: 0; color: var(--mdc-dim); }
  .branch-name span { color: var(--mdc-fg-soft); font-family: var(--mdc-mono); font-size: 12px; overflow-wrap: anywhere; }
  .branch-name :global(svg) { flex-shrink: 0; }
  small { font-size: 8px; font-weight: 600; letter-spacing: .06em; border: 1px solid var(--mdc-border-strong); padding: 1px 5px; border-radius: 4px; color: var(--mdc-muted); }
  .state { display: flex; align-items: center; font-size: 11px; color: var(--mdc-muted); }
  .online { color: var(--mdc-accent-down); }
  .open { display: inline-flex; align-items: center; justify-content: center; gap: 7px; padding: 6px 10px; color: var(--mdc-fg-soft); background: var(--mdc-card); border: 1px solid var(--mdc-border-strong); border-radius: 6px; font-size: 11px; text-decoration: none; }
  .open:hover { background: var(--mdc-card-hover); color: var(--mdc-accent-strong); border-color: var(--mdc-accent); }
  a:focus-visible { outline: 2px solid var(--mdc-accent); outline-offset: 3px; }
  .unavailable { color: var(--mdc-border-strong); text-align: center; }
  .error { padding: 14px 18px; margin-bottom: 18px; border-radius: var(--mdc-radius-sm); background: color-mix(in srgb, var(--mdc-error) 7%, var(--mdc-panel)); color: var(--mdc-error); overflow-wrap: anywhere; }
  .error span { display: block; margin-top: 4px; font-size: 12px; }
  .empty { display: flex; flex-direction: column; gap: 14px; align-items: center; padding: 65px 20px; text-align: center; color: var(--mdc-muted); border: 1px dashed var(--mdc-border-strong); border-radius: var(--mdc-radius-md); }
  .empty p { margin: 0; font-size: 12px; }
  footer { display: flex; justify-content: space-between; color: var(--mdc-muted); font-size: 10px; margin-top: 24px; }
  @media (max-width: 700px) {
    .topbar { padding: 0 18px; }
    main { padding: 32px 18px 24px; }
    .intro { align-items: flex-start; flex-direction: column; gap: 20px; }
    h1 { font-size: 29px; }
    .controls { flex-wrap: wrap; gap: 10px; }
    .search { flex-basis: calc(100% - 46px); max-width: none; }
    .filters { order: 2; margin-left: 0; }
    .branch-row { grid-template-columns: minmax(0, 1fr) 76px 66px; gap: 8px; padding: 12px 14px; }
    .project-head { padding: 14px; }
    .branch-name { gap: 8px; }
    small { display: none; }
  }
</style>
