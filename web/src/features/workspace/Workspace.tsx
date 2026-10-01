import logo from "../../assets/mdc-logo.svg?no-inline";
import { useEffect, useLayoutEffect, useState } from "react";
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
import { shortFnode } from "../../lib/format";
import "./workspace.css";

export default function Workspace({ onReady }: { onReady?: () => void }) {
  const [controller] = useState(() => new WorkspaceController({ onReady }));
  const s = useModel(controller),
    session = useModel(nodeSession),
    workspace = useModel(s.workspaceSession);
  useLayoutEffect(() => {
    s.props = { onReady };
    s.reconcile();
  });
  useEffect(() => {
    session.prepareEditor = s.prepareEditor;
    const stopTheme = s.watchTheme(),
      stopListening = s.listen(),
      stopStatuses = s.watchStatuses();
    s.start();
    document.title = `${s.project} · MathDoc`;
    return () => {
      s.cancelStartup();
      session.cancel();
      session.prepareEditor = undefined;
      stopListening();
      stopTheme();
      stopStatuses();
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
  const Graph = s.GraphComponent;
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
            <img src={logo} alt="" />
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
          title="Search nodes"
        >
          <Search size={14} />
          <span>Search nodes</span>
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
        <div className="workspace-panels">
          <div className="context-panel" id="context">
            <NodeColumn
              title="Referrers"
              accent="up"
              items={session.referrers}
              lastVisitedFnode={session.lastVisitedFnode}
              context={session.node?.fnode ?? null}
              active={s.view === "columns"}
              onSelect={(id) => void select(id)}
            />
          </div>
          <div className="node-panel" id="editor">
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
                  ref={s.attachEditor}
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
          </div>
          <div className="dependencies-panel" id="dependencies">
            <div
              className={`graph-panel ${s.view !== "force" ? "hidden" : ""}`}
            >
              {Graph && (
                <Graph
                  ref={s.attachGraph}
                  active={s.view === "force"}
                  theme={s.theme}
                  onSelect={s.onForceSelect}
                  selectedFnode={session.selectedFnode}
                  revision={s.graphRevision}
                />
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
          </div>
        </div>
      </main>
      <footer className="statusbar">
        <span className="status-uuid">
          {s.statusLoad.kind === "ready" ? shortFnode(s.statusLoad.node.fnode) : ""}
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
