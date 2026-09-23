<script lang="ts">
  import { onMount } from "svelte";
  import { ArrowUpRight, FolderOpen, GitBranch, GitBranchPlus, Moon, Play, Plus, RefreshCw, Search, Square, Sun, Trash2 } from "@lucide/svelte";
  import { projectsApi, isAbortError, type ServiceStatus } from "./lib/api";
  import ProjectCreate from "./components/ProjectCreate.svelte";
  import { applyTheme, currentTheme, observeTheme, type Theme } from "./lib/theme";

  let { onReady }: { onReady?: () => void } = $props();

  let status = $state<ServiceStatus | null>(null);
  let error = $state<string | null>(null);
  let refreshing = $state(false);
  let updated = $state("");
  let query = $state("");
  let filter = $state<"all" | "running" | "stopped">("all");
  let theme = $state<Theme>(currentTheme());
  let create = $state<{source?: string} | null>(null);
  let pending = $state<Record<string, string>>({});
  let failures = $state<Record<string, string>>({});
  let refreshTask: Promise<void> | undefined;
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

  function refresh() {
    return refreshTask ??= load().finally(() => { refreshTask = undefined; });
  }
  async function load() {
    refreshing = true;
    try {
      status = await projectsApi.list(AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]));
      updated = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      error = null;
    } catch (e) {
      if (!isAbortError(e)) error = e instanceof Error ? e.message : String(e);
    } finally { refreshing = false; onReady?.(); }
  }
  async function changed() {
    // Let an older poll finish before requesting the post-mutation inventory.
    await refreshTask;
    await refresh();
  }
  async function manage(project: string, action: "start" | "stop" | "delete_branch") {
    if (pending[project] || pending[project.split("/")[0]!]) return;
    if (action === "delete_branch" && !confirm(`Delete ${project} and its Lean caches?`)) return;
    pending[project] = action; delete failures[project];
    try { await projectsApi.change({action, project}); }
    catch (e) { failures[project] = e instanceof Error ? e.message : String(e); }
    finally { await changed(); delete pending[project]; }
  }
  function projectPending(database: string) {
    return !!pending[database] || branches.some(row => row.database === database && !!pending[row.name]);
  }
  async function removeProject(database: string) {
    if (projectPending(database)) return;
    if (!confirm(`Delete entire project ${database}?\n\nAll its branches will be stopped. All project data, history, Lean caches and unsaved editor sessions will be deleted.`)) return;
    pending[database] = "remove"; delete failures[database];
    try { await projectsApi.change({action: "remove", database}); }
    catch (e) { failures[database] = e instanceof Error ? e.message : String(e); }
    finally { await changed(); delete pending[database]; }
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
  <header class="app-header">
    <a class="app-brand" href="/" aria-label="MathDoc projects"><img src="/mdc-logo.svg" alt="" /><strong>MathDoc</strong></a>
    <button class="header-refresh" class:spinning={refreshing} onclick={() => void refresh()} disabled={refreshing} title="Refresh projects" aria-label="Refresh projects"><RefreshCw size={16} strokeWidth={1.8} /></button>
    <button class="header-theme" onclick={toggleTheme} title="Toggle theme" aria-label="Toggle theme">
      {#if theme === "dark"}<Sun size={16} strokeWidth={1.8} />{:else}<Moon size={16} strokeWidth={1.8} />{/if}
    </button>
  </header>

  <div class="page-head">
    <div class="head-content">
      <div class="intro">
        <h1>Projects</h1>
        <div class="overview" aria-label="Project summary">
          <span><strong>{projectCount}</strong> {projectCount === 1 ? "project" : "projects"}</span><span><strong>{branches.length}</strong> {branches.length === 1 ? "branch" : "branches"}</span>
          <span class="running-total"><i></i><strong>{running}</strong> running</span>
        </div>
        <button class="init" onclick={() => create = {}} title="Initialize a project" aria-label="Init project"><Plus size={16} /></button>
      </div>

      <div class="controls">
        <label class="search"><Search size={17} strokeWidth={1.8} /><input aria-label="Search projects and branches" placeholder="Search projects or branches..." bind:value={query} /></label>
        <div class="filters" aria-label="Filter branches">
          <button aria-pressed={filter === "all"} onclick={() => filter = "all"}>All branches</button>
          <button aria-pressed={filter === "running"} onclick={() => filter = "running"}>Running</button>
          <button aria-pressed={filter === "stopped"} onclick={() => filter = "stopped"}>Stopped</button>
        </div>
      </div>
    </div>
  </div>

  <main>
    <div class="list-content">
      {#if error}<div class="error" role="alert">Could not refresh projects: {error}{#if status}<span>Showing the last known states.</span>{/if}</div>{/if}
      {#each Object.entries(failures).filter(([name]) => !name.includes("/")) as [database, message] (database)}
        <div class="error" role="alert">Could not delete project {database}: {message}</div>
      {/each}
      {#if !status && !error}
        <div class="empty" role="status">Loading projects...</div>
      {:else if status && branches.length === 0}
        <div class="empty"><FolderOpen size={30} /><h2>No projects yet</h2></div>
      {:else if status && groups.length === 0}
        <div class="empty"><Search size={28} /><h2>No matching branches</h2><p>Try a different name or change the status filter.</p></div>
      {:else}
        <div class="projects">
          {#each groups as [database, rows] (database)}
            <section class="project" aria-label={`Project ${database}`}>
              <div class="project-head">
                <FolderOpen size={18} strokeWidth={1.7} /><h2>{database}</h2><span>{rows.length} {rows.length === 1 ? "branch" : "branches"}</span>
                <button class="icon-button delete" disabled={projectPending(database)} onclick={() => void removeProject(database)} title={`Delete entire project ${database}`} aria-label={`Delete project ${database}`}>
                  {#if pending[database]}<RefreshCw size={16} class="spinning" />{:else}<Trash2 size={16} />{/if}
                </button>
              </div>
              {#each rows as row (row.name)}
                <div class="branch-row" data-project={row.name}>
                  <span class="branch-name" title={row.name}><GitBranch size={16} strokeWidth={1.6} /><span class:main-branch={row.branch === "main"} title={row.branch === "main" ? "Main branch: can only be deleted with the entire project" : undefined}>{row.branch}</span></span>
                  <span class="state" class:online={row.running}><i></i>{row.running ? "Running" : "Stopped"}</span>
                  <span class="counts" aria-label={`Graph size of ${row.name}`}>
                    <span>{#if row.running && row.nodes !== undefined}<strong>{row.nodes.toLocaleString()}</strong> {row.nodes === 1 ? "node" : "nodes"}{/if}</span>
                    <span>{#if row.running && row.edges !== undefined}<strong>{row.edges.toLocaleString()}</strong> {row.edges === 1 ? "edge" : "edges"}{/if}</span>
                  </span>
                  <div class="actions" aria-label={`Manage ${row.name}`}>
                    <button class="toggle" class:stopped={!row.running} disabled={!!pending[row.name] || !!pending[database]} onclick={() => void manage(row.name, row.running ? "stop" : "start")} title={`${row.running ? "Stop" : "Start"} ${row.name}`} aria-label={`${row.running ? "Stop" : "Start"} ${row.name}`}>
                      {#if pending[row.name]}<RefreshCw size={14} class="spinning" />{:else if row.running}<Square size={13} />{:else}<Play size={14} />{/if}
                    </button>
                    <div class="branch-actions" class:main={row.branch === "main"}>
                      <button class="icon-button" disabled={!!pending[row.name] || !!pending[database]} onclick={() => create = {source: row.name}} title={`New branch from ${row.name}`} aria-label={`New branch from ${row.name}`}><GitBranchPlus size={16} /></button>
                      {#if row.branch !== "main"}
                        <button class="icon-button delete" disabled={row.running || !!pending[row.name] || !!pending[database]} onclick={() => void manage(row.name, "delete_branch")} title={row.running ? "Stop this branch before deleting" : `Delete ${row.name}`} aria-label={`Delete ${row.name}`}><Trash2 size={15} /></button>
                      {/if}
                    </div>
                    {#if row.running && row.url && !pending[row.name] && !pending[database]}
                      <a class="open icon-button" href={`/p/${row.name}/`} title={`Open ${row.name}`} aria-label={`Open ${row.name}`}><ArrowUpRight size={17} /></a>
                    {:else}<span class="open-space" aria-hidden="true"></span>{/if}
                  </div>
                  {#if failures[row.name]}<div class="row-error" role="alert">{failures[row.name]}</div>{/if}
                </div>
              {/each}
            </section>
          {/each}
        </div>
      {/if}
      {#if updated}<footer>Last refreshed {updated}<span>Updates automatically</span></footer>{/if}
    </div>
  </main>
</div>
{#if create}<ProjectCreate source={create.source} onClose={() => create = null} onCreated={async () => {query = ""; filter = "all"; await changed();}} />{/if}

<style>
  .directory { height: 100%; display: flex; flex-direction: column; overflow: hidden; }
  .header-refresh { display: grid; place-items: center; flex: 0 0 32px; width: 32px; height: 32px; padding: 0; margin-left: auto; color: var(--mdc-dim); background: transparent; border: 1px solid transparent; border-radius: var(--mdc-radius-sm); }
  .header-refresh:hover:not(:disabled) { color: var(--mdc-fg); background: var(--mdc-card-hover); }
  .header-refresh.spinning :global(svg) { animation: spin 1s linear infinite; }
  .app-header :global(.header-theme) { margin-left: 0; }
  .page-head { flex: none; border-bottom: 1px solid var(--mdc-border); background: color-mix(in srgb, var(--mdc-panel) 82%, transparent); backdrop-filter: blur(12px) saturate(160%); }
  .head-content { max-width: 1144px; margin: 0 auto; padding: 30px 32px 22px; }
  main { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .list-content { max-width: 1144px; margin: 0 auto; padding: 24px 32px 28px; }
  .intro { display: flex; align-items: center; gap: 24px; margin-bottom: 28px; }
  h1 { font-size: 34px; line-height: 1.15; font-weight: 630; letter-spacing: -.035em; margin: 0; }
  .overview { display: flex; align-items: center; gap: 19px; color: var(--mdc-dim); font-size: 12px; white-space: nowrap; margin-left: auto; }
  .init { display: inline-flex; align-items: center; justify-content: center; width: 34px; height: 34px; padding: 0; flex-shrink: 0; border: 1px solid color-mix(in srgb, var(--mdc-accent) 45%, var(--mdc-border)); border-radius: var(--mdc-radius-sm); color: var(--mdc-accent); background: color-mix(in srgb, var(--mdc-accent) 8%, var(--mdc-panel)); }
  .init:hover { background: color-mix(in srgb, var(--mdc-accent) 12%, transparent); }
  .overview strong { font-size: 14px; font-weight: 550; color: var(--mdc-fg-soft); margin-right: 3px; }
  .running-total { display: flex; align-items: center; }
  i { display: inline-block; width: 6px; height: 6px; border-radius: 50%; background: var(--mdc-muted); margin-right: 8px; }
  .running-total i, .online i { background: var(--mdc-accent-down); box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-accent-down) 9%, transparent); }
  .controls { display: flex; align-items: center; gap: 12px; }
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
  .branch-row { display: grid; grid-template-columns: minmax(100px, 1fr) 90px 208px auto; gap: 16px; align-items: center; min-height: 66px; padding: 12px 20px; }
  .branch-row + .branch-row { border-top: 1px solid var(--mdc-border); }
  .branch-name { display: flex; align-items: center; gap: 12px; min-width: 0; color: var(--mdc-dim); }
  .branch-name span { color: var(--mdc-fg-soft); font-family: var(--mdc-mono); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .branch-name .main-branch { padding: 2px 7px; border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); }
  .branch-name :global(svg) { flex-shrink: 0; }
  .state { display: flex; align-items: center; font-size: 11px; color: var(--mdc-muted); }
  .online { color: var(--mdc-accent-down); }
  .counts { display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; min-height: 18px; font-size: 11px; color: var(--mdc-muted); text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
  .counts strong { color: var(--mdc-fg-soft); font-weight: 500; }
  .actions { display: flex; align-items: center; justify-content: flex-end; gap: 8px; min-width: 148px; }
  .toggle { display: inline-flex; align-items: center; justify-content: center; width: 34px; height: 32px; padding: 0; color: var(--mdc-dim); background: var(--mdc-card); border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); }
  .toggle:hover:not(:disabled) { color: var(--mdc-fg); background: var(--mdc-card-hover); }
  .toggle.stopped { color: var(--mdc-accent-down); }
  .branch-actions { display: grid; grid-template-columns: repeat(2, 30px); padding: 1px; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-sm); }
  .branch-actions.main { grid-template-columns: 30px; margin-right: 30px; }
  .branch-actions .icon-button { width: 30px; height: 28px; }
  .delete { color: var(--mdc-error); }
  .delete:hover:not(:disabled) { color: var(--mdc-error); background: color-mix(in srgb, var(--mdc-error) 12%, transparent); }
  .open { text-decoration: none; color: var(--mdc-fg-soft); border-color: var(--mdc-border-strong); background: var(--mdc-card); }
  .open:hover { background: var(--mdc-card-hover); color: var(--mdc-accent-strong); border-color: var(--mdc-accent); }
  a:focus-visible { outline: 2px solid var(--mdc-accent); outline-offset: 3px; }
  .open-space { width: 34px; flex-shrink: 0; }
  .row-error { grid-column: 1 / -1; color: var(--mdc-error); font-size: 12px; overflow-wrap: anywhere; }
  .project-head :global(.spinning), .actions :global(.spinning) { animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .error { padding: 14px 18px; margin-bottom: 18px; border-radius: var(--mdc-radius-sm); background: color-mix(in srgb, var(--mdc-error) 7%, var(--mdc-panel)); color: var(--mdc-error); overflow-wrap: anywhere; }
  .error span { display: block; margin-top: 4px; font-size: 12px; }
  .empty { display: flex; flex-direction: column; gap: 14px; align-items: center; padding: 65px 20px; text-align: center; color: var(--mdc-muted); border: 1px dashed var(--mdc-border-strong); border-radius: var(--mdc-radius-md); }
  .empty p { margin: 0; font-size: 12px; }
  footer { display: flex; justify-content: space-between; color: var(--mdc-muted); font-size: 10px; margin-top: 24px; }
  @media (max-width: 800px) {
    .head-content { padding: 22px 18px 16px; }
    .list-content { padding: 20px 18px 24px; }
    .intro { flex-wrap: wrap; gap: 16px; }
    .overview { order: 3; flex-basis: 100%; margin-left: 0; gap: 16px; }
    .init { margin-left: auto; }
    h1 { font-size: 29px; }
    .controls { flex-wrap: wrap; gap: 10px; }
    .search { flex-basis: calc(100% - 46px); max-width: none; }
    .filters { order: 2; margin-left: 0; }
    .branch-row { grid-template-columns: minmax(0, 1fr) auto; gap: 12px; padding: 14px; }
    .counts { grid-column: 1 / -1; grid-template-columns: repeat(2, 96px); text-align: left; }
    .actions { grid-column: 1 / -1; justify-content: flex-end; }
    .project-head { padding: 14px; }
    .branch-name { gap: 8px; }
  }
</style>
