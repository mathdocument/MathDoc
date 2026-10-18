<script lang="ts">
  // The collaboration page (docs/mathdoc.md §4.8), at /p/DATABASE/BRANCH/collab: a workbench of
  // sections, the problem with the current section, and an inspector, as wide as one drags it,
  // where the selected goal (overview, notes, certificates) or certificate is read and changed.
  // People use the same commands as agents.
  import { onDestroy, onMount, untrack } from "svelte";
  import { ArrowLeft, ClipboardCheck, Hammer, ListTodo, ListTree, Moon, Settings2, Sun } from "@lucide/svelte";
  import "./collab.css";
  import { Collab, type InspectorTab, type Section, type Selection } from "../../lib/collab.svelte";
  import { stages } from "../../lib/collab-layout";
  import { goalStatement, openGoals, type CollabView } from "../../lib/coordination";
  import { projectName, projectPath } from "../../lib/project-path";
  import { applyTheme, currentTheme, observeTheme, type Theme } from "../../lib/theme";
  import Account from "../Account.svelte";
  import MarkdownPreview from "../MarkdownPreview.svelte";
  import AssembleSection from "./AssembleSection.svelte";
  import GoalStatus from "./GoalStatus.svelte";
  import Inspector from "./Inspector.svelte";
  import ProjectSection from "./ProjectSection.svelte";
  import ProofGraph from "./ProofGraph.svelte";
  import ProofTreeView from "./ProofTreeView.svelte";
  import ReviewSection from "./ReviewSection.svelte";
  import TasksSection from "./TasksSection.svelte";
  let { onReady }: { onReady?: () => void } = $props();

  const params = new URLSearchParams(location.search);
  const c = new Collab(params.get("board"));
  const project = projectName();
  const SECTIONS: Section[] = ["tree", "review", "tasks", "assemble", "project"];
  const TABS: InspectorTab[] = ["overview", "notes", "certificates"];
  // `section=notes` was the Notes section, now the inspector's Notes tab beside the proof tree.
  const asked = params.get("section");
  let section = $state<Section>(SECTIONS.includes(asked as Section) ? (asked as Section) : "tree");
  let treeView = $state<"tree" | "graph">(params.get("view") === "graph" ? "graph" : "tree");
  let selection = $state<Selection>(params.get("goal") ? { kind: "goal", goal: params.get("goal")! } : null);
  let tab = $state<InspectorTab>(asked === "notes" ? "notes" : TABS.includes(params.get("tab") as InspectorTab) ? (params.get("tab") as InspectorTab) : "overview");
  let theme = $state<Theme>(currentTheme());
  let problemOpen = $state(false);
  let problemTall = $state(false);
  let problemEl = $state<HTMLDivElement>();
  let center = $state<HTMLDivElement>();
  // The inspector's width, dragged at its edge and kept in this browser.
  const WIDTH_KEY = "mdc-inspector-width";
  const clampWidth = (w: number) => Math.round(Math.min(Math.max(w, 300), Math.max(300, innerWidth * 0.6)));
  let inspectorWidth = $state(clampWidth(Number(localStorage.getItem(WIDTH_KEY)) || 448));
  const nav = $derived([
    { key: "tree" as const, label: "Proof tree", icon: ListTree, count: 0, title: "" },
    { key: "review" as const, label: "Review", icon: ClipboardCheck, count: c.proposed.length, title: "Decompositions awaiting the owner's review" },
    { key: "tasks" as const, label: "Tasks", icon: ListTodo, count: c.openTasks.length, title: "Tasks whose goal has no proof yet" },
    { key: "assemble" as const, label: "Assemble & write back", icon: Hammer, count: c.awaiting, title: "Certified assemblies not written back yet" },
    { key: "project" as const, label: "Project", icon: Settings2, count: 0, title: "" },
  ]);
  const sectionOf: Record<CollabView, Section> = { overview: "project", tasks: "tasks", decompositions: "review", review: "assemble" };
  const labels: Record<Section, string> = { tree: "Proof tree", review: "Review", tasks: "Tasks", assemble: "Assemble & write back", project: "Project" };
  const stageList = $derived(c.board?.project && c.branch ? stages(c.board, c.branch.branch) : []);
  const rootStatus = $derived(c.board?.root ? c.statusOf(c.board.root) : null);
  const documentUrl = $derived(projectPath(`/${c.board?.project?.root_node ? `#ref=${c.board.project.root_node}` : ""}`));

  // The section, view, goal, tab and board are in the address, so a link opens the same place.
  $effect(() => {
    const q = new URLSearchParams();
    if (c.boardId && params.get("board")) q.set("board", c.boardId);
    if (section !== "tree") q.set("section", section);
    if (treeView === "graph") q.set("view", "graph");
    if (selection?.kind === "goal") q.set("goal", selection.goal);
    if (selection?.kind === "goal" && tab !== "overview") q.set("tab", tab);
    const search = q.toString();
    history.replaceState(history.state, "", `${location.pathname}${search ? `?${search}` : ""}`);
  });
  // Another section starts at the top of the column, under the problem.
  $effect(() => {
    void section;
    untrack(() => center?.scrollTo({ top: 0 }));
  });
  // Start on the goal the proof waits for, else the root.
  $effect(() => {
    const b = c.board;
    if (!b?.root) return;
    untrack(() => {
      if (selection) return;
      const open = c.view ? openGoals(b, c.view.picks) : [];
      selection = { kind: "goal", goal: open[0] ?? b.root };
    });
  });
  // A long problem folds to a few lines; the fold shows only when the text is taller.
  $effect(() => {
    void c.rootText;
    const el = problemEl;
    if (!el) return;
    const measure = () => { problemTall = el.scrollHeight > el.clientHeight + 4 || problemOpen; };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  });
  function resize(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    const move = (e: PointerEvent) => { inspectorWidth = clampWidth(innerWidth - e.clientX); };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      localStorage.setItem(WIDTH_KEY, String(inspectorWidth));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up, { once: true });
  }
  function nudge(event: KeyboardEvent) {
    const d = event.key === "ArrowLeft" ? 24 : event.key === "ArrowRight" ? -24 : 0;
    if (!d) return;
    event.preventDefault();
    inspectorWidth = clampWidth(inspectorWidth + d);
    localStorage.setItem(WIDTH_KEY, String(inspectorWidth));
  }
  function openNode(fnode: string) {
    location.assign(projectPath(`/#ref=${encodeURIComponent(fnode)}`));
  }
  function toggleTheme() {
    theme = theme === "dark" ? "light" : "dark";
    applyTheme(theme);
  }
  onMount(() => {
    document.title = `Collaboration · ${project ?? "MathDoc"}`;
    void c.start().finally(() => onReady?.());
    return observeTheme((next) => { theme = next; applyTheme(next, false); });
  });
  onDestroy(() => c.stop());
</script>

<div class="collab-page">
  <header class="app-header">
    <a class="app-brand" href="/" aria-label="All projects" title="All projects"><img src="/mdc-logo.svg" alt="" /><strong>MathDoc</strong></a>
    <a class="crumb" href={documentUrl} title="Back to the document"><ArrowLeft size={15} strokeWidth={1.9} />{project}</a>
    <span class="crumb-sep">/</span>
    <span class="crumb-here">Collaboration</span>
    {#if c.boards.length > 1}
      <select aria-label="Proof request" value={c.boardId} onchange={(e) => c.selectBoard((e.currentTarget as HTMLSelectElement).value)}>
        {#each c.boards as b (b.id)}<option value={b.id}>{b.title}</option>{/each}
      </select>
    {/if}
    <span style="flex: 1"></span>
    <button class="header-theme" onclick={toggleTheme} title="Toggle theme" aria-label="Toggle theme">
      {#if theme === "dark"}<Sun size={16} strokeWidth={1.8} />{:else}<Moon size={16} strokeWidth={1.8} />{/if}
    </button>
    <Account />
  </header>

  <main class="collab-main" aria-label="Collaboration">
    <div class="workbench" style:--inspector-width={`${inspectorWidth}px`}>
      <nav class="sections" aria-label="Sections">
        {#each nav as n (n.key)}
          <button aria-current={section === n.key ? "page" : undefined} onclick={() => (section = n.key)}>
            <n.icon size={16} strokeWidth={1.8} /><span class="label">{n.label}</span>{#if n.count}<span class="badge" title={n.title}>{n.count}</span>{/if}
          </button>
        {/each}
      </nav>

      <div class="center" bind:this={center}>
        <section class="hero" aria-label="Problem">
          {#if c.board}
            <p class="eyebrow">{c.board.project ? `Project of ${c.board.project.database}` : "Proof request"} · owner {c.board.owner} · {c.board.members.length} {c.board.members.length === 1 ? "member" : "members"}</p>
            <h1>{c.board.title} {#if rootStatus}<GoalStatus status={rootStatus} />{/if}</h1>
            {#if c.rootText}
              <div class="problem" class:folded={!problemOpen} bind:this={problemEl}><MarkdownPreview compact source={c.rootText} /></div>
              {#if problemTall}<button class="link problem-toggle" onclick={() => (problemOpen = !problemOpen)}>{problemOpen ? "Show less" : "Show the whole problem"}</button>{/if}
            {:else if c.board.root}
              <pre class="lean" style="max-width: 62rem; margin: 0 0 1rem">{goalStatement(c.board, c.board.root)}</pre>
            {/if}
            {#if stageList.length}
              <ol class="stages" aria-label="Stages">
                {#each stageList as s, i (s.key)}
                  <li data-state={s.state} aria-current={s.state === "current" ? "step" : undefined}><span class="dot">{s.state === "done" ? "✓" : i + 1}</span><span>{s.label}</span><span class="detail">{s.detail}</span></li>
                {/each}
              </ol>
            {/if}
          {:else}
            <h1>Collaboration</h1>
            <p class="muted">{c.loaded && !c.boards.length ? "No project in this database yet. Open the root theorem's Lean node and use “Submit to LeanGround”." : "Loading…"}</p>
          {/if}
        </section>
        {#if c.step}
          <p class="next" data-tone={c.step.tone} role="status" aria-label="Next step">
            <strong>{c.step.tone === "done" ? "Done" : c.step.tone === "wait" ? "Waiting" : "Next"}:</strong> {c.step.text}
            {#if c.step.view && sectionOf[c.step.view] !== section}<button class="link" onclick={() => (section = sectionOf[c.step!.view!])}>Open {labels[sectionOf[c.step.view]]} →</button>{/if}
          </p>
        {/if}
        {#if c.loadError}<p class="error" role="status">{c.loadError}</p>{/if}
        {#if c.board}
          {#if section === "tree"}
            <div class="section-head">
              <h2>Proof tree</h2>
              <div class="seg" role="group" aria-label="Proof view">
                <button aria-pressed={treeView === "tree"} onclick={() => (treeView = "tree")}>Outline</button>
                <button aria-pressed={treeView === "graph"} onclick={() => (treeView = "graph")}>Graph</button>
              </div>
            </div>
            {#if treeView === "tree"}
              <ProofTreeView {c} {selection} onSelect={(s) => (selection = s)} />
            {:else}
              <ProofGraph {c} {selection} onSelect={(s) => (selection = s)} />
            {/if}
          {:else if section === "review"}
            <ReviewSection {c} onSelect={(s) => (selection = s)} />
          {:else if section === "tasks"}
            <TasksSection {c} onSelect={(s) => (selection = s)} />
          {:else if section === "assemble"}
            <AssembleSection {c} onOpenNode={openNode} />
          {:else}
            <ProjectSection {c} onOpenNode={openNode} />
          {/if}
        {/if}
      </div>

      <!-- A focusable separator is the ARIA window splitter; the arrow keys resize it too. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <div class="resizer" role="separator" aria-orientation="vertical" aria-label="Inspector width" aria-valuenow={inspectorWidth} aria-valuemin={300} tabindex="0"
        title="Drag to resize the inspector" onpointerdown={resize} onkeydown={nudge}></div>
      <Inspector {c} {selection} onSelect={(s) => (selection = s)} bind:tab />
    </div>
  </main>
  <!-- A refused command shows over every column, so it is seen wherever it was sent from. -->
  {#if c.refused}<p class="refused" role="alert">{c.refused} <button class="dismiss" onclick={() => (c.refused = null)} aria-label="Dismiss">×</button></p>{/if}
</div>
