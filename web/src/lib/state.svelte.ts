import type { NodeDetail, NodeInfo, NodeView } from "./types";
import { api } from "./api";
import {
  confirmDiscardDrafts,
  settlePendingMutations,
  unsavedDraftRevision,
} from "./unsaved";
import {
  browserHistoryEntry,
  browserHistoryTarget,
  focusedHistoryState,
  type BrowserHistoryEntry,
  type BrowserHistoryMode,
} from "./history";

export { browserHistoryEntry, browserHistoryTarget } from "./history";
export type { BrowserHistoryEntry, BrowserHistoryMode } from "./history";

export type LoadState =
  | { kind: "idle" }
  | { kind: "ready"; node: NodeDetail }
  | { kind: "error"; message: string };

export interface ColumnState {
  items: NodeInfo[];
  selected: number; // -1 = none
}

interface NavigateOptions {
  pushHistory?: boolean;
  direction?: "up" | "down" | "neutral";
  skipTransition?: boolean;
  skipUnsavedGuard?: boolean;
  historyIndex?: number;
  historyEntries?: string[];
  browserHistory?: BrowserHistoryMode;
  forceDiscovery?: boolean;
}

export class NodeSession {
  snapshot = $state<NodeView | null>(null);
  selectionCleared = $state(false);
  history = $state<string[]>([]);
  historyIdx = $state(-1);
  editorRevision = $state(0);
  /** fnode of the previously focused node — highlighted in columns. */
  lastVisitedFnode = $state<string | null>(null);
  navigationError = $state<string | null>(null);
  failedNavigationFnode = $state<string | null>(null);
  referrersSelected = $state(-1);
  childrenSelected = $state(-1);
  private loadError = $state<string | null>(null);
  private navigationRequest = 0;
  private syncRequest = 0;

  get load(): LoadState {
    if (this.snapshot) return { kind: "ready", node: this.snapshot.node };
    return this.loadError ? { kind: "error", message: this.loadError } : { kind: "idle" };
  }

  get selectedLoad(): LoadState {
    return this.selectionCleared ? { kind: "idle" } : this.load;
  }

  get node(): NodeDetail | null {
    return this.snapshot?.node ?? null;
  }

  get selectedFnode(): string | null {
    return this.selectionCleared ? null : this.node?.fnode ?? null;
  }

  get referrers(): ColumnState {
    return { items: this.snapshot?.referrers ?? [], selected: this.referrersSelected };
  }

  get children(): ColumnState {
    return { items: this.snapshot?.children ?? [], selected: this.childrenSelected };
  }

  cancel(): void {
    this.navigationRequest++;
    this.syncRequest++;
  }

  acceptNode(node: NodeDetail): void {
    if (!this.snapshot || this.snapshot.node.fnode !== node.fnode) return;
    this.syncRequest++;
    this.snapshot = { ...this.snapshot, node };
  }

  async select(fnode: string, opts: NavigateOptions = {}): Promise<boolean> {
    if (!opts.skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    const request = ++this.navigationRequest;
    this.syncRequest++;
    if (!await settlePendingMutations()) return false;
    if (request !== this.navigationRequest) return false;

    const confirmedDraftRevision = unsavedDraftRevision();
    try {
      const view = await api.nodeView(fnode, opts.forceDiscovery);
      let committed = false;
      const apply = () => {
        if (request !== this.navigationRequest || committed) return;
        if (unsavedDraftRevision() !== confirmedDraftRevision) return;
        const leaving = this.node?.fnode ?? null;
        if (leaving !== view.node.fnode) this.lastVisitedFnode = leaving;
        this.editorRevision++;
        this.snapshot = view;
        this.selectionCleared = false;
        this.loadError = null;
        this.referrersSelected = -1;
        this.childrenSelected = -1;
        commitFocusedHistory(view.node.fnode, opts);
        this.navigationError = null;
        this.failedNavigationFnode = null;
        committed = true;
      };
      if (opts.skipTransition) apply();
      else await withViewTransition(opts.direction ?? "neutral", apply);
      return committed;
    } catch (error) {
      if (request !== this.navigationRequest ||
        unsavedDraftRevision() !== confirmedDraftRevision) return false;
      this.navigationError = error instanceof Error ? error.message : String(error);
      this.failedNavigationFnode = fnode;
      if (!this.snapshot) this.loadError = this.navigationError;
      return false;
    }
  }

  async clearSelection(opts: NavigateOptions = {}): Promise<boolean> {
    if (!opts.skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    const request = ++this.navigationRequest;
    this.syncRequest++;
    if (!await settlePendingMutations()) return false;
    if (request !== this.navigationRequest) return false;
    const confirmedDraftRevision = unsavedDraftRevision();
    let committed = false;
    await withViewTransition("neutral", () => {
      if (request !== this.navigationRequest ||
        unsavedDraftRevision() !== confirmedDraftRevision) return;
      this.editorRevision++;
      this.selectionCleared = true;
      commitClearedHistory(opts);
      this.navigationError = null;
      this.failedNavigationFnode = null;
      committed = true;
    }, "force-editor");
    return committed;
  }

  async syncView(fnode = this.node?.fnode): Promise<boolean> {
    if (!fnode || this.node?.fnode !== fnode) return false;
    const request = ++this.syncRequest;
    const revision = this.node.revision;
    const draftRevision = unsavedDraftRevision();
    try {
      const view = await api.nodeView(fnode);
      if (request !== this.syncRequest || this.node?.fnode !== fnode ||
        this.node.revision !== revision || unsavedDraftRevision() !== draftRevision) return false;
      this.snapshot = view;
      this.referrersSelected = -1;
      this.childrenSelected = -1;
      return true;
    } catch (error) {
      if (request !== this.syncRequest || this.node?.fnode !== fnode) return false;
      throw error;
    }
  }
}

export const nodeSession = new NodeSession();
export const appState = nodeSession;

/** True if the current browser supports the View Transitions API. */
function supportsViewTransitions(): boolean {
  return typeof document !== "undefined" &&
    typeof (document as Document & { startViewTransition?: unknown }).startViewTransition === "function";
}

/**
 * Apply a state mutation through the View Transitions API when available,
 * otherwise run it synchronously. The callback must perform all reactive
 * updates that should be part of the transition.
 */
let viewTransitionToken = 0;
let activeViewTransition: { skipTransition: () => void } | null = null;

export async function withViewTransition(
  direction: "up" | "down" | "neutral",
  mutate: () => void,
  scope?: string,
): Promise<void> {
  if (!supportsViewTransitions()) {
    mutate();
    return;
  }
  activeViewTransition?.skipTransition();
  const token = ++viewTransitionToken;
  let applied = false;
  const apply = () => {
    if (applied) return;
    applied = true;
    mutate();
  };
  const cleanup = () => {
    if (token !== viewTransitionToken) return;
    activeViewTransition = null;
    delete document.documentElement.dataset.vtDirection;
    delete document.documentElement.dataset.vtScope;
  };
  document.documentElement.dataset.vtDirection = direction;
  if (scope) document.documentElement.dataset.vtScope = scope;
  else delete document.documentElement.dataset.vtScope;
  try {
    const vt = (document as Document & {
      startViewTransition: (cb: () => void) => {
        finished: Promise<void>;
        updateCallbackDone: Promise<void>;
        skipTransition: () => void;
      };
    }).startViewTransition(apply);
    activeViewTransition = vt;
    void vt.finished.then(cleanup).catch(cleanup);
    await vt.updateCallbackDone;
  } catch {
    cleanup();
    apply();
  }
}

export function cancelNavigation(): void {
  nodeSession.cancel();
}

export function initialHistoryOptions(fnode: string): {
  pushHistory: boolean;
  historyIndex?: number;
  historyEntries?: string[];
  browserHistory: BrowserHistoryMode;
} {
  const entry = browserHistoryEntry(window.history.state);
  if (entry && browserHistoryTarget(entry) === fnode) {
    return {
      pushHistory: false,
      historyIndex: entry.index,
      historyEntries: entry.entries,
      browserHistory: "replace",
    };
  }
  return { pushHistory: true, browserHistory: "replace" };
}

export function commitFocusedHistory(
  fnode: string,
  opts: {
    pushHistory?: boolean;
    historyIndex?: number;
    historyEntries?: string[];
    browserHistory?: BrowserHistoryMode;
  } = {},
): void {
  const push = opts.pushHistory ?? true;
  const history = focusedHistoryState(appState.history, appState.historyIdx, fnode, opts);
  appState.history = history.entries;
  appState.historyIdx = history.index;

  const mode = opts.browserHistory ?? (push ? "push" : "none");
  if (mode === "none") return;
  const url = new URL(window.location.href);
  url.hash = new URLSearchParams({ ref: fnode }).toString();
  const state: BrowserHistoryEntry = {
    mdcHistory: 1,
    fnode,
    index: appState.historyIdx,
    entries: [...appState.history],
  };
  if (mode === "push") {
    window.history.pushState(state, "", url);
  } else {
    window.history.replaceState(state, "", url);
  }
}

export function commitClearedHistory(
  opts: {
    pushHistory?: boolean;
    historyIndex?: number;
    historyEntries?: string[];
    browserHistory?: BrowserHistoryMode;
  } = {},
): void {
  const backingEntries = opts.historyEntries ?? appState.history;
  const backingIndex = opts.historyIndex ?? appState.historyIdx;
  const backingFnode = backingEntries[backingIndex];
  const url = new URL(window.location.href);
  url.hash = "";

  if (!backingFnode) {
    window.history.replaceState(null, "", url);
    return;
  }

  const push = opts.pushHistory ?? true;
  const history = focusedHistoryState(appState.history, appState.historyIdx, backingFnode, opts);
  appState.history = history.entries;
  appState.historyIdx = history.index;

  const mode = opts.browserHistory ?? (push ? "push" : "none");
  if (mode === "none") return;
  const state: BrowserHistoryEntry = {
    mdcHistory: 1,
    fnode: null,
    index: appState.historyIdx,
    entries: [...appState.history],
  };
  if (mode === "push") {
    window.history.pushState(state, "", url);
  } else {
    window.history.replaceState(state, "", url);
  }
}

/** Navigate to a node by fnode. Updates the center, both columns, and history. */
export async function navigate(
  fnode: string,
  opts: NavigateOptions = {},
): Promise<boolean> {
  return nodeSession.select(fnode, opts);
}

export function clearSelection(opts: NavigateOptions = {}): Promise<boolean> {
  return nodeSession.clearSelection(opts);
}

/** Refresh only the focused node detail after a write (no view transition). */
export function refreshFocused(node: NodeDetail) {
  nodeSession.acceptNode(node);
}

export function canGoBack(): boolean {
  return appState.historyIdx > 0;
}

export function canGoForward(): boolean {
  return appState.historyIdx < appState.history.length - 1;
}
