import { lazy, Suspense, useEffect, useLayoutEffect, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Columns3,
  GitBranch,
  Link2,
  Moon,
  Network,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Sun,
  Trash2,
  Unlink2,
} from "lucide-react";
import { Group, Panel, Separator, useGroupRef } from "react-resizable-panels";
import { nodeSession } from "../../lib/node-session";
import { useModel } from "../../hooks/use-model";
import { Button, IconButton } from "../../components/ui/button";
import EditorPane from "../editor/EditorPane";
import { NodeColumn } from "./NodeColumn";
import { WorkspaceController } from "./controller";
import {
  NewNodeDialog,
  RemoveDependenciesDialog,
  SearchDialog,
} from "./NodeDialogs";
import { ProjectSettings } from "./ProjectSettings";
import { readLayout, saveLayout } from "./layout";
import "./workspace.css";
const Graph = lazy(() => import("../graph/Graph"));

export default function Workspace({ onReady }: { onReady?: () => void }) {
  const [controller] = useState(() => new WorkspaceController({ onReady }));
  const s = useModel(controller),
    session = useModel(nodeSession),
    workspace = useModel(s.workspaceSession);
  const group = useGroupRef();
  const [initialLayout] = useState(() => readLayout("columns"));
  useLayoutEffect(() => {
    s.props = { onReady };
    s.reconcile();
  });
  useLayoutEffect(() => {
    const frame = requestAnimationFrame(() =>
      group.current?.setLayout(readLayout(s.view)),
    );
    return () => cancelAnimationFrame(frame);
  }, [group, s.view]);
  useEffect(() => {
    const stopTheme = s.watchTheme(),
      stopListening = s.listen();
    s.start();
    document.title = `${s.project} · MathDoc`;
    return () => {
      s.cancelStartup();
      session.cancel();
      stopListening();
      stopTheme();
    };
  }, [s, session]);
  const busy = s.refreshing || s.historyNavigating || s.changingView;
  const closeOverlay = () => {
    s.overlay = { kind: "none" };
  };
  const select = async (id: string) => {
    s.cancelStartup();
    if (await session.select(id)) closeOverlay();
  };
  const active = s.activeNode;
  return (
    <div className="app" data-view={s.view} inert={busy} aria-busy={busy}>
      <header className="app-header">
        <div className="brand-zone">
          <a
            className="app-brand"
            href="/"
            aria-label="All projects"
            title="All projects"
            onClick={(event) => void s.showProjects(event.nativeEvent)}
          >
            <img src="/mdc-logo.svg" alt="" />
            <strong>MathDoc</strong>
          </a>
          <span className="project-breadcrumb">
            <GitBranch size={12} />
            {s.project}
          </span>
        </div>
        <button
          className="search-tool"
          onClick={() => {
            s.overlay = { kind: "search" };
          }}
          title="Search nodes (/)"
        >
          <Search size={14} />
          <span>Search nodes</span>
          <kbd>/</kbd>
        </button>
        <div className="workspace-tools">
          <div className="tool-cluster" aria-label="node history">
            <IconButton
              label="Back"
              variant="quiet"
              onClick={() => window.history.back()}
              disabled={session.historyIdx <= 0}
            >
              <ArrowLeft size={15} />
            </IconButton>
            <IconButton
              label="Forward"
              variant="quiet"
              onClick={() => window.history.forward()}
              disabled={
                session.historyIdx < 0 ||
                session.historyIdx >= session.history.length - 1
              }
            >
              <ArrowRight size={15} />
            </IconButton>
          </div>
          <span className="toolbar-divider" />
          <IconButton
            label="Create node"
            variant="quiet"
            onClick={() => {
              s.overlay = { kind: "new-node" };
            }}
          >
            <Plus size={16} />
          </IconButton>
          <div className="tool-cluster" aria-label="dependency actions">
            <IconButton
              label="Add dependency"
              variant="quiet"
              disabled={!s.activeReady}
              onClick={() => {
                if (s.activeFnode)
                  s.overlay = { kind: "add-dep", target: s.activeFnode };
              }}
            >
              <Link2 size={15} />
            </IconButton>
            <IconButton
              label="Remove dependency"
              variant="quiet"
              disabled={!s.activeReady || !s.activeDepens.length}
              onClick={() => {
                if (s.activeFnode)
                  s.overlay = { kind: "rm-dep", target: s.activeFnode };
              }}
            >
              <Unlink2 size={15} />
            </IconButton>
          </div>
          <IconButton
            label="Delete node"
            variant="quiet"
            className="danger"
            disabled={!s.activeReady}
            onClick={() => void s.deleteActiveNode()}
          >
            <Trash2 size={15} />
          </IconButton>
          <span className="toolbar-divider" />
          <div className="segmented view-switch" aria-label="workspace view">
            <button
              aria-pressed={s.view === "columns"}
              title="Knowledge view"
              onClick={() => {
                if (s.view !== "columns") void s.toggleGraphView();
              }}
            >
              <Columns3 size={14} />
              <span>Knowledge</span>
            </button>
            <button
              aria-pressed={s.view === "force"}
              title="Graph view"
              onClick={() => {
                if (s.view !== "force") void s.toggleGraphView();
              }}
            >
              <Network size={14} />
              <span>Graph</span>
            </button>
          </div>
          <span className="toolbar-divider" />
          <IconButton
            label="Project settings"
            variant="quiet"
            onClick={() => {
              s.overlay = { kind: "project" };
            }}
          >
            <Settings size={16} />
          </IconButton>
          <IconButton
            label="Refresh database view"
            variant="quiet"
            className={s.refreshing ? "spinning" : ""}
            onClick={() => void s.refreshView()}
          >
            <RefreshCw size={15} />
          </IconButton>
          <IconButton
            label={`Switch to ${s.theme === "dark" ? "light" : "dark"} mode`}
            variant="quiet"
            onClick={s.toggleTheme}
          >
            {s.theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
          </IconButton>
        </div>
      </header>
      {(session.navigationError || s.refreshError) && (
        <div className="app-error" role="alert">
          <span>{session.navigationError ?? s.refreshError}</span>
          <Button onClick={s.retryNavigation}>retry</Button>
        </div>
      )}
      <main className="workspace-main">
        <Group
          groupRef={group}
          defaultLayout={initialLayout}
          disabled={s.view === "columns"}
          onLayoutChanged={(layout, meta) => {
            if (s.view === "force" && meta.isUserInteraction)
              saveLayout(s.view, layout);
          }}
          orientation="horizontal"
          className="workspace-panels"
          resizeTargetMinimumSize={{ coarse: 20, fine: 6 }}
        >
          <Panel
            className="context-panel"
            id="context"
            defaultSize="22%"
            minSize={s.view === "force" ? "0%" : "15%"}
            maxSize={s.view === "force" ? "0%" : "70%"}
            collapsible
          >
            <NodeColumn
              title="Referrers"
              accent="up"
              items={session.referrers}
              lastVisitedFnode={session.lastVisitedFnode}
              context={session.node?.fnode ?? null}
              active={s.view === "columns"}
              onSelect={(id) => void select(id)}
            />
          </Panel>
          <Separator
            className={`panel-separator ${s.view === "force" ? "hidden" : ""}`}
            disabled
            aria-label="Referrers and editor boundary"
          />
          <Panel
            className="node-panel"
            id="editor"
            defaultSize="56%"
            minSize="30%"
          >
            <div className="editor-wrap">
              {s.initialError && s.view === "columns" ? (
                <div className="empty-state">
                  <Network size={30} />
                  <strong>A new graph starts here</strong>
                  <p>{s.initialError}</p>
                  <Button
                    variant="primary"
                    onClick={() => {
                      s.overlay = { kind: "new-node" };
                    }}
                  >
                    <Plus size={15} />
                    Create node
                  </Button>
                </div>
              ) : (
                <EditorPane
                  load={
                    s.view === "force" ? session.selectedLoad : session.load
                  }
                  theme={s.theme}
                  selection={session.editorRevision}
                  latexPreview={session.latexPreview}
                  onPreviewChange={(value) => {
                    session.latexPreview = value;
                  }}
                  onRefresh={s.refreshNode}
                  onReady={() => {
                    s.editorReady = true;
                  }}
                  latexTarget={s.latexTarget}
                  onLatexNavigate={s.navigateLatex}
                />
              )}
            </div>
          </Panel>
          <Separator
            className="panel-separator"
            disabled={s.view === "columns"}
            aria-label="Resize node and graph"
          />
          <Panel
            className="dependencies-panel"
            id="dependencies"
            defaultSize="22%"
            minSize="15%"
            maxSize={s.view === "force" ? "70%" : "35%"}
          >
            <div
              className={`graph-panel ${s.view !== "force" ? "hidden" : ""}`}
            >
              {s.graphModule && (
                <Suspense
                  fallback={
                    <div className="empty-state" role="status">
                      Loading graph...
                    </div>
                  }
                >
                  <Graph
                    ref={s.attachGraph}
                    active={s.view === "force"}
                    theme={s.theme}
                    onSelect={s.onForceSelect}
                    selectedFnode={session.selectedFnode}
                    revision={s.graphRevision}
                  />
                </Suspense>
              )}
            </div>
            <NodeColumn
              title="Dependencies"
              accent="down"
              items={session.children}
              lastVisitedFnode={session.lastVisitedFnode}
              context={session.node?.fnode ?? null}
              active={s.view === "columns"}
              onSelect={(id) => void select(id)}
            />
          </Panel>
        </Group>
      </main>
      <footer className="statusbar">
        <span className="status-uuid">
          {s.statusLoad.kind === "ready" ? s.statusLoad.node.fnode : ""}
        </span>
        <span className="status-node">
          {s.statusLoad.kind === "ready"
            ? s.statusLoad.node.name
            : s.statusLoad.kind === "error"
              ? s.statusLoad.message
              : "No selection"}
        </span>
        <span
          className={`graph-stats ${workspace.error ? "error" : ""}`}
          title={workspace.title}
          aria-live="polite"
        >
          <i />
          {workspace.report
            ? `${workspace.report.nodes.toLocaleString()} nodes · ${workspace.report.edges.toLocaleString()} edges`
            : workspace.loading
              ? "Checking graph..."
              : "Graph unavailable"}
        </span>
        <span className="status-shortcuts">
          <kbd>/</kbd> search <kbd>g</kbd> graph
        </span>
      </footer>
      {s.overlay.kind === "new-node" && (
        <NewNodeDialog onCreated={s.afterNodeCreated} onClose={closeOverlay} />
      )}
      {s.overlay.kind === "search" && (
        <SearchDialog
          onPick={(id) => void select(id)}
          onAdded={s.afterDepMutation}
          onClose={closeOverlay}
        />
      )}
      {s.overlay.kind === "add-dep" && active && (
        <SearchDialog
          target={active}
          onPick={(id) => void select(id)}
          onAdded={s.afterDepMutation}
          onClose={closeOverlay}
        />
      )}
      {s.overlay.kind === "rm-dep" && active && (
        <RemoveDependenciesDialog
          target={active}
          children={session.children}
          onRemoved={s.afterDepMutation}
          onClose={closeOverlay}
        />
      )}
      {s.overlay.kind === "project" && (
        <ProjectSettings onClose={closeOverlay} />
      )}
    </div>
  );
}
