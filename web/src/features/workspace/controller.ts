import { nodeSession } from "../../lib/node-session";
import { withViewTransition } from "../../lib/view-transition";
import {
  browserHistoryEntry,
  browserHistoryTarget,
  type BrowserHistoryEntry,
  type FocusedHistoryOptions,
} from "../../lib/history";
import { WorkspaceSession } from "../../lib/workspace-session";
import { settleEditorLayout } from "../../lib/editor-layout";
import { api } from "../../lib/api";
import { errMsg } from "../../lib/format";
import { projectName } from "../../lib/project-path";
import type { NodeDetail } from "../../lib/types";
import {
  confirmDiscardDrafts,
  hasUnsavedDrafts,
  settlePendingMutations,
  trackMutation,
} from "../../lib/unsaved";
import {
  applyTheme,
  currentTheme,
  observeTheme,
  type Theme,
} from "../../lib/theme";
import { ObservableModel } from "../../lib/observable";
export interface WorkspaceControllerProps {
  onReady?: () => void;
}
type Overlay =
  | {
      kind: "none";
    }
  | {
      kind: "search";
    }
  | {
      kind: "add-dep";
      target: string;
    }
  | {
      kind: "rm-dep";
      target: string;
    }
  | {
      kind: "new-node";
    }
  | {
      kind: "project";
    };
export class WorkspaceController extends ObservableModel {
  props: WorkspaceControllerProps;
  get onReady() {
    return this.props.onReady;
  }
  editorReady = false;
  workspaceReady = false;
  overlay: Overlay = { kind: "none" };
  latexTarget: {
    fnode: string;
    label: string;
  } | null = null;
  theme: Theme = currentTheme();
  startupRequest = 0;
  initialNavigationRetry: {
    fnode: string;
    clearedEntry: BrowserHistoryEntry | null;
  } | null = null;
  initialError: string | null = null;
  refreshError: string | null = null;
  refreshing = false;
  refreshRequest = 0;
  historyNavigating = false;
  workspaceSession = new WorkspaceSession();
  project = projectName();
  view: "columns" | "force" = "columns";
  changingView = false;
  depthGraph:
    | {
        prepare: () => Promise<void>;
      }
    | undefined;
  GraphComponent: typeof import("../graph/Graph").default | null = null;
  graphRevision = 0;
  get activeFnode() {
    return this.view === "force"
      ? nodeSession.selectedFnode
      : (nodeSession.node?.fnode ?? null);
  }
  get activeNode() {
    return this.view === "force" && nodeSession.selectionCleared
      ? null
      : nodeSession.node;
  }
  get activeReady() {
    return this.activeFnode !== null && this.activeNode !== null;
  }
  get activeDepens() {
    return this.activeNode?.depens ?? [];
  }
  get statusLoad() {
    return this.view === "force" ? nodeSession.selectedLoad : nodeSession.load;
  }
  navigateLatex = async (fnode: string, label: string) => {
    this.cancelStartup();
    if (await nodeSession.select(fnode)) this.latexTarget = { fnode, label };
  };
  showProjects = async (event: MouseEvent) => {
    if (
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    )
      return;
    event.preventDefault();
    if (!confirmDiscardDrafts() || !(await settlePendingMutations())) return;
    window.location.assign("/");
  };
  cancelStartup = () => {
    this.startupRequest++;
  };
  toggleTheme = () => {
    this.theme = this.theme === "dark" ? "light" : "dark";
    applyTheme(this.theme);
  };
  findDefaultFnode = async (): Promise<string | null> => {
    const roots = await api.roots();
    if (roots.length > 0) {
      return roots.sort((a, b) => b.topo_depth - a.topo_depth)[0]!.fnode;
    }
    return (await api.full()).nodes[0]?.fnode ?? null;
  };
  navigateInitial = async (
    fnode: string,
    clearedEntry: BrowserHistoryEntry | null = null,
  ): Promise<boolean> => {
    const committed = await nodeSession.select(fnode, {
      ...nodeSession.initialHistoryOptions(fnode),
      skipTransition: true,
    });
    if (committed && clearedEntry) {
      nodeSession.commitClearedHistory({
        pushHistory: false,
        historyIndex: clearedEntry.index,
        historyEntries: clearedEntry.entries,
        browserHistory: "replace",
      });
    }
    return committed;
  };
  refreshView = async () => {
    if (this.refreshing) return;
    if (!confirmDiscardDrafts()) return;
    const request = ++this.refreshRequest;
    this.cancelStartup();
    this.refreshError = null;
    this.refreshing = true;
    try {
      if (!(await settlePendingMutations())) return;
      if (request !== this.refreshRequest) return;
      const checked = await this.workspaceSession.refresh();
      if (request !== this.refreshRequest) return;
      const selectionWasCleared = nodeSession.selectionCleared;
      const current = nodeSession.node;
      let refreshed = false;
      if (current) {
        refreshed = await nodeSession.select(current.fnode, {
          pushHistory: false,
          skipTransition: true,
          skipUnsavedGuard: true,
          clearOnNotFound: true,
        });
        if (request !== this.refreshRequest) return;
      }
      if (!nodeSession.node) {
        const defaultFnode = await this.findDefaultFnode();
        if (request !== this.refreshRequest) return;
        if (defaultFnode) {
          refreshed = await nodeSession.select(defaultFnode, {
            skipTransition: true,
            skipUnsavedGuard: true,
            browserHistory: "replace",
          });
        } else {
          this.initialError =
            "This project has no nodes. Create one with the + button.";
          refreshed = true;
        }
      }
      if (selectionWasCleared && this.view === "force")
        nodeSession.selectionCleared = true;
      if (request !== this.refreshRequest) return;
      if (refreshed || checked) this.graphRevision++;
      if (!refreshed) this.refreshError = "refresh request failed";
    } catch (error) {
      if (request === this.refreshRequest) {
        this.refreshError =
          error instanceof Error ? error.message : String(error);
      }
    } finally {
      this.refreshing = false;
    }
  };
  refreshNode = (node: NodeDetail, graphChanged = false) => {
    nodeSession.acceptNode(node);
    if (graphChanged) {
      this.graphRevision++;
      void nodeSession.syncView(node.fnode).catch((error) => {
        this.refreshError =
          error instanceof Error ? error.message : String(error);
      });
    }
  };
  afterDepMutation = (
    updated: NodeDetail,
    delta: {
      nodes: number;
      edges: number;
    },
  ) => {
    this.refreshError = null;
    this.workspaceSession.applyDelta(delta.nodes, delta.edges);
    this.refreshNode(updated, true);
  };
  afterNodeCreated = (fnode: string, skipUnsavedGuard = false) => {
    this.cancelStartup();
    this.initialError = null;
    this.graphRevision++;
    this.workspaceSession.applyDelta(1, 0);
    if (this.historyNavigating) return;
    if (this.view === "force")
      void this.onForceSelect(fnode, { skipUnsavedGuard });
    else void nodeSession.select(fnode, { skipUnsavedGuard });
  };
  deleteActiveNode = async () => {
    const node = this.activeNode;
    if (!node || this.refreshing) return;
    if (
      !window.confirm(
        `Delete “${node.name}”? This removes the node and all dependencies pointing to it. Unsaved edits to this node will also be discarded.`,
      )
    )
      return;
    this.refreshing = true;
    this.refreshError = null;
    let clearMutation: (() => void) | undefined;
    try {
      if (!(await settlePendingMutations())) return;
      this.cancelStartup();
      clearMutation = trackMutation();
      const result = await api.deleteNode(
        node.fnode,
        this.activeNode?.revision ?? node.revision,
      );
      clearMutation();
      nodeSession.removeNode(node.fnode);
      this.graphRevision++;
      this.workspaceSession.applyDelta(-1, -result.removed_edges);
      const next = await this.findDefaultFnode();
      if (next) {
        await nodeSession.select(next, {
          skipUnsavedGuard: true,
          browserHistory: "replace",
        });
      } else {
        this.initialError =
          "This project has no nodes. Create one with the + button.";
      }
    } catch (error) {
      this.refreshError = errMsg(error);
    } finally {
      clearMutation?.();
      this.refreshing = false;
    }
  };
  onForceSelect = async (
    fnode: string | null,
    opts: FocusedHistoryOptions & {
      skipUnsavedGuard?: boolean;
    } = {},
  ): Promise<boolean> => {
    this.cancelStartup();
    return fnode
      ? nodeSession.select(fnode, opts)
      : nodeSession.clearSelection(opts);
  };
  toggleGraphView = async () => {
    if (this.refreshing || this.historyNavigating || this.changingView) return;
    this.cancelStartup();
    nodeSession.cancel();
    this.changingView = true;
    try {
      if (this.view === "columns" && !this.GraphComponent)
        this.GraphComponent = (await import("../graph/Graph")).default;
      // Retain the same editor and LSP; reveal only after the new width is measured.
      await withViewTransition(
        () => {
          if (this.view === "columns") {
            const entry = browserHistoryEntry(window.history.state);
            nodeSession.selectionCleared =
              entry?.fnode === null &&
              browserHistoryTarget(entry) === nodeSession.node?.fnode;
            this.view = "force";
          } else {
            nodeSession.selectionCleared = false;
            this.view = "columns";
          }
        },
        "ready",
        async () => {
          await Promise.resolve();
          if (this.view === "force") await this.depthGraph?.prepare();
          await settleEditorLayout();
        },
      );
    } catch (error) {
      this.refreshError = errMsg(error);
    } finally {
      this.changingView = false;
    }
  };
  watchTheme() {
    return observeTheme((nextTheme) => {
      this.theme = nextTheme;
      applyTheme(nextTheme, false);
    });
  }
  listen() {
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!hasUnsavedDrafts()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    let restoringHistory = false;
    let popstateRequest = 0;
    const onPopState = async (event: PopStateEvent) => {
      const request = ++popstateRequest;
      this.cancelStartup();
      nodeSession.cancel();
      this.refreshRequest++;
      if (restoringHistory) {
        restoringHistory = false;
        this.historyNavigating = false;
        return;
      }
      const entry = browserHistoryEntry(event.state);
      if (!entry) return;
      this.historyNavigating = true;
      let awaitingRollback = false;
      try {
        if (request !== popstateRequest) return;
        const activeEntry = browserHistoryEntry(window.history.state);
        if (
          !activeEntry ||
          activeEntry.index !== entry.index ||
          activeEntry.fnode !== entry.fnode
        )
          return;
        const previousIndex = nodeSession.historyIdx;
        const target = browserHistoryTarget(entry);
        const committed =
          this.view === "force"
            ? await this.onForceSelect(entry.fnode, {
                pushHistory: false,
                historyIndex: entry.index,
                historyEntries: entry.entries,
                browserHistory: "replace",
              })
            : await nodeSession.select(target, {
                pushHistory: false,
                historyIndex: entry.index,
                historyEntries: entry.entries,
                browserHistory: "replace",
              });
        if (request !== popstateRequest) return;
        if (committed) {
          if (this.view === "columns" && entry.fnode === null) {
            nodeSession.commitClearedHistory({
              pushHistory: false,
              historyIndex: entry.index,
              historyEntries: entry.entries,
              browserHistory: "replace",
            });
          }
          this.overlay = { kind: "none" };
          return;
        }
        const currentEntry = browserHistoryEntry(window.history.state);
        if (
          !currentEntry ||
          currentEntry.index !== entry.index ||
          currentEntry.fnode !== entry.fnode ||
          nodeSession.historyIdx !== previousIndex
        )
          return;
        const delta = previousIndex - entry.index;
        if (delta !== 0) {
          restoringHistory = true;
          awaitingRollback = true;
          window.history.go(delta);
        }
      } finally {
        if (request === popstateRequest && !awaitingRollback)
          this.historyNavigating = false;
      }
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    window.addEventListener("popstate", onPopState);
    return () => {
      this.workspaceSession.cancel();
      window.removeEventListener("beforeunload", onBeforeUnload);
      window.removeEventListener("popstate", onPopState);
    };
  }
  start() {
    void (async () => {
      const request = ++this.startupRequest;
      const isCurrent = () => request === this.startupRequest;
      try {
        // URL hash can override the default even when a cyclic graph has no roots.
        const hash = window.location.hash.slice(1);
        const params = new URLSearchParams(hash);
        const currentEntry = browserHistoryEntry(window.history.state);
        const hashRef = params.get("ref");
        const clearedEntry =
          hashRef === null && currentEntry?.fnode === null
            ? currentEntry
            : null;
        const ref =
          hashRef ??
          (currentEntry?.fnode === null
            ? browserHistoryTarget(currentEntry)
            : null);
        if (ref) {
          const resolved = await api.resolve(ref);
          if (!isCurrent()) return;
          const committed = await this.navigateInitial(
            resolved.fnode,
            clearedEntry,
          );
          if (isCurrent() && committed && params.get("label"))
            this.latexTarget = {
              fnode: resolved.fnode,
              label: params.get("label")!,
            };
          if (isCurrent() && !committed) {
            this.initialNavigationRetry = {
              fnode: resolved.fnode,
              clearedEntry,
            };
          }
          return;
        }
        const defaultFnode = await this.findDefaultFnode();
        if (!isCurrent()) return;
        if (!defaultFnode) {
          this.initialError =
            "This project has no nodes. Create one with the + button.";
          return;
        }
        const committed = await this.navigateInitial(defaultFnode);
        if (isCurrent() && !committed) {
          this.initialNavigationRetry = {
            fnode: defaultFnode,
            clearedEntry: null,
          };
        }
      } catch (e) {
        if (isCurrent())
          this.initialError = e instanceof Error ? e.message : String(e);
      } finally {
        await this.workspaceSession.refresh();
        this.workspaceReady = true;
      }
    })();
  }
  syncLatexTarget() {
    const current =
      this.view === "force"
        ? nodeSession.selectedFnode
        : nodeSession.node?.fnode;
    if (this.latexTarget && current && this.latexTarget.fnode !== current)
      this.latexTarget = null;
  }
  syncReady() {
    if (this.workspaceReady && (this.initialError || this.editorReady))
      this.onReady?.();
    if (nodeSession.load.kind === "ready") {
      this.initialError = null;
      this.initialNavigationRetry = null;
    }
  }
  syncOverlay() {
    if ("target" in this.overlay && this.activeFnode !== this.overlay.target) {
      this.overlay = { kind: "none" };
    }
  }
  attachGraph = (graph: { prepare: () => Promise<void> } | null) => {
    this.depthGraph = graph ?? undefined;
  };
  reconcile() {
    this.syncLatexTarget();
    this.syncReady();
    this.syncOverlay();
  }
  retryNavigation = () => {
    const target = nodeSession.failedNavigationFnode;
    if (!target) {
      void this.refreshView();
      return;
    }
    const initial =
      this.initialNavigationRetry?.fnode === target
        ? this.initialNavigationRetry
        : null;
    this.cancelStartup();
    if (initial) void this.navigateInitial(target, initial.clearedEntry);
    else if (this.view === "force") void this.onForceSelect(target);
    else
      void nodeSession.select(target, {
        pushHistory: nodeSession.node?.fnode !== target,
      });
  };
  constructor(props: WorkspaceControllerProps) {
    super();
    this.props = props;
    this.observe(
      "editorReady",
      "workspaceReady",
      "overlay",
      "latexTarget",
      "theme",
      "initialError",
      "refreshError",
      "refreshing",
      "historyNavigating",
      "view",
      "changingView",
      "GraphComponent",
      "graphRevision",
    );
  }
}
