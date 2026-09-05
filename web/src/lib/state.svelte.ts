import { browserHistoryAdapter, type BrowserHistoryAdapter } from "./browser-history";
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
  type FocusedHistoryOptions,
} from "./history";

export { browserHistoryEntry, browserHistoryTarget } from "./history";
export type { BrowserHistoryEntry, BrowserHistoryMode, FocusedHistoryOptions } from "./history";

export type LoadState =
  | { kind: "idle" }
  | { kind: "ready"; node: NodeDetail }
  | { kind: "error"; message: string };

export interface ColumnState {
  items: NodeInfo[];
  selected: number; // -1 = none
}

interface NavigateOptions extends FocusedHistoryOptions {
  skipTransition?: boolean;
  skipUnsavedGuard?: boolean;
  clearOnError?: boolean;
}

export class NodeSession {
  constructor(private readonly browserHistory: BrowserHistoryAdapter = browserHistoryAdapter) {}

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
      const view = await api.nodeView(fnode);
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
        this.commitFocusedHistory(view.node.fnode, opts);
        this.navigationError = null;
        this.failedNavigationFnode = null;
        committed = true;
      };
      if (opts.skipTransition) apply();
      else await withViewTransition(apply);
      return committed;
    } catch (error) {
      if (request !== this.navigationRequest ||
        unsavedDraftRevision() !== confirmedDraftRevision) return false;
      this.navigationError = error instanceof Error ? error.message : String(error);
      this.failedNavigationFnode = fnode;
      if (opts.clearOnError) this.snapshot = null;
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
    await withViewTransition(() => {
      if (request !== this.navigationRequest ||
        unsavedDraftRevision() !== confirmedDraftRevision) return;
      this.editorRevision++;
      this.selectionCleared = true;
      this.commitClearedHistory(opts);
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
    try {
      const view = await api.nodeView(fnode);
      const node = this.node;
      if (request !== this.syncRequest || node?.fnode !== fnode ||
        node.revision !== revision) return false;
      if (view.node.revision !== revision) {
        throw new Error(`${fnode} changed externally; refresh before continuing`);
      }
      this.snapshot = { node, referrers: view.referrers, children: view.children };
      this.referrersSelected = -1;
      this.childrenSelected = -1;
      return true;
    } catch (error) {
      if (request !== this.syncRequest || this.node?.fnode !== fnode) return false;
      throw error;
    }
  }
  initialHistoryOptions(fnode: string): FocusedHistoryOptions {
    const entry = browserHistoryEntry(this.browserHistory.state());
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

  commitFocusedHistory(
    fnode: string,
    opts: FocusedHistoryOptions = {},
  ): void {
    const push = opts.pushHistory ?? true;
    const previousIndex = this.historyIdx;
    const history = focusedHistoryState(this.history, this.historyIdx, fnode, opts);
    this.history = history.entries;
    this.historyIdx = history.index;

    const mode = opts.browserHistory ?? (push ? previousIndex < 0 ? "replace" : "push" : "none");
    if (mode === "none") return;
    const state: BrowserHistoryEntry = {
      mdcHistory: 1,
      fnode,
      index: this.historyIdx,
      entries: [...this.history],
    };
    this.browserHistory.commit(mode, fnode, state);
  }

  commitClearedHistory(
    opts: FocusedHistoryOptions = {},
  ): void {
    const backingEntries = opts.historyEntries ?? this.history;
    const backingIndex = opts.historyIndex ?? this.historyIdx;
    const backingFnode = backingEntries[backingIndex];

    if (!backingFnode) {
      this.browserHistory.commit("replace", null, null);
      return;
    }

    const push = opts.pushHistory ?? true;
    const history = focusedHistoryState(this.history, this.historyIdx, backingFnode, opts);
    this.history = history.entries;
    this.historyIdx = history.index;

    const mode = opts.browserHistory ?? (push ? "push" : "none");
    if (mode === "none") return;
    const state: BrowserHistoryEntry = {
      mdcHistory: 1,
      fnode: null,
      index: this.historyIdx,
      entries: [...this.history],
    };
    this.browserHistory.commit(mode, null, state);
  }

}

export const nodeSession = new NodeSession();

/**
 * Apply a state mutation through the View Transitions API when available,
 * otherwise run it synchronously. The callback must perform all reactive
 * updates that should be part of the transition.
 */
let viewTransitionToken = 0;
let activeViewTransition: ViewTransition | null = null;

export async function withViewTransition(
  mutate: () => void,
  scope?: string,
): Promise<void> {
  if (typeof document === "undefined" || typeof document.startViewTransition !== "function") {
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
    delete document.documentElement.dataset.vtScope;
  };
  if (scope) document.documentElement.dataset.vtScope = scope;
  else delete document.documentElement.dataset.vtScope;
  try {
    const vt = document.startViewTransition(apply);
    activeViewTransition = vt;
    void vt.finished.then(cleanup).catch(cleanup);
    await vt.updateCallbackDone;
  } catch {
    cleanup();
    apply();
  }
}
