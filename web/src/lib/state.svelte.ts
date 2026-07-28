import type { NodeDetail, NodeInfo } from "./types";
import { api } from "./api";
import {
  confirmDiscardDrafts,
  settlePendingMutations,
  unsavedDraftRevision,
} from "./unsaved";

export type LoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; node: NodeDetail }
  | { kind: "error"; message: string };

export interface ColumnState {
  items: NodeInfo[];
  selected: number; // -1 = none
}

function emptyColumn(): ColumnState {
  return { items: [], selected: -1 };
}

export const appState = $state({
  load: { kind: "idle" } as LoadState,
  referrers: emptyColumn(),
  children: emptyColumn(),
  history: [] as string[],
  historyIdx: -1,
  editorRevision: 0,
  /** fnode of the previously focused node — highlighted in columns. */
  lastVisitedFnode: null as string | null,
  navigationError: null as string | null,
  failedNavigationFnode: null as string | null,
});

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
async function withViewTransition(
  direction: "up" | "down" | "neutral",
  mutate: () => void,
): Promise<void> {
  if (!supportsViewTransitions()) {
    mutate();
    return;
  }
  document.body.dataset.vtDirection = direction;
  try {
    const vt = (document as Document & {
      startViewTransition: (cb: () => void) => {
        finished: Promise<void>;
        updateCallbackDone: Promise<void>;
      };
    }).startViewTransition(() => {
      mutate();
    });
    void vt.finished.then(() => {
      delete document.body.dataset.vtDirection;
    }).catch(() => {
      delete document.body.dataset.vtDirection;
    });
    await vt.updateCallbackDone;
  } catch {
    delete document.body.dataset.vtDirection;
    mutate();
  }
}

let navigationRequest = 0;

export type BrowserHistoryMode = "push" | "replace" | "none";

export interface BrowserHistoryEntry {
  mdcHistory: 1;
  fnode: string;
  index: number;
  entries: string[];
}

export function browserHistoryEntry(value: unknown): BrowserHistoryEntry | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<BrowserHistoryEntry>;
  if (
    candidate.mdcHistory !== 1 ||
    typeof candidate.fnode !== "string" ||
    !Number.isInteger(candidate.index) ||
    !Array.isArray(candidate.entries) ||
    !candidate.entries.every((entry) => typeof entry === "string") ||
    candidate.index! < 0 ||
    candidate.index! >= candidate.entries.length ||
    candidate.entries[candidate.index!] !== candidate.fnode
  ) {
    return null;
  }
  return candidate as BrowserHistoryEntry;
}

export function initialHistoryOptions(fnode: string): {
  pushHistory: boolean;
  historyIndex?: number;
  browserHistory: BrowserHistoryMode;
} {
  const entry = browserHistoryEntry(window.history.state);
  if (entry?.fnode === fnode) {
    appState.history = [...entry.entries];
    appState.historyIdx = entry.index;
    return {
      pushHistory: false,
      historyIndex: entry.index,
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
    browserHistory?: BrowserHistoryMode;
  } = {},
): void {
  const push = opts.pushHistory ?? true;
  if (push) {
    appState.history = [
      ...appState.history.slice(0, appState.historyIdx + 1),
      fnode,
    ];
    appState.historyIdx = appState.history.length - 1;
  } else if (opts.historyIndex !== undefined) {
    appState.historyIdx = opts.historyIndex;
  }

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

/** Navigate to a node by fnode. Updates the center, both columns, and history. */
export async function navigate(
  fnode: string,
  opts: {
    pushHistory?: boolean;
    direction?: "up" | "down" | "neutral";
    skipTransition?: boolean;
    skipUnsavedGuard?: boolean;
    historyIndex?: number;
    browserHistory?: BrowserHistoryMode;
  } = {},
): Promise<boolean> {
  if (!opts.skipUnsavedGuard && !confirmDiscardDrafts()) return false;
  if (!await settlePendingMutations()) return false;

  const confirmedDraftRevision = unsavedDraftRevision();
  const request = ++navigationRequest;
  const push = opts.pushHistory ?? true;
  const direction = opts.direction ?? "neutral";
  const skipTransition = opts.skipTransition ?? false;

  // Fetch new node data while keeping the old node visible.
  // The old content stays on screen until the View Transition snapshot
  // is taken (inside withViewTransition's callback), so there's no flash.
  try {
    const view = await api.nodeView(fnode);

    let committed = false;
    const apply = () => {
      if (request !== navigationRequest || committed) return;
      if (unsavedDraftRevision() !== confirmedDraftRevision) return;
      const leaving = appState.load.kind === "ready" ? appState.load.node.fnode : null;
      appState.lastVisitedFnode = leaving;
      appState.editorRevision++;
      appState.load = { kind: "ready", node: view.node };
      appState.referrers = { items: view.referrers, selected: -1 };
      appState.children = { items: view.children, selected: -1 };
      commitFocusedHistory(fnode, {
        pushHistory: push,
        historyIndex: opts.historyIndex,
        browserHistory: opts.browserHistory,
      });
      committed = true;
      appState.navigationError = null;
      appState.failedNavigationFnode = null;
    };

    if (skipTransition) {
      apply();
    } else {
      await withViewTransition(direction, apply);
    }
    return committed;
  } catch (e) {
    if (request !== navigationRequest) return false;
    appState.navigationError = e instanceof Error ? e.message : String(e);
    appState.failedNavigationFnode = fnode;
    // Keep an existing editor mounted on navigation failure; replacing it with
    // an error page could discard drafts created while the request was pending.
    if (appState.load.kind === "ready") return false;
    appState.load = {
      kind: "error",
      message: appState.navigationError,
    };
    return false;
  }
}

/** Refresh only the focused node detail after a write (no view transition). */
export function refreshFocused(node: NodeDetail) {
  // Replace the ready node in place so the editor doesn't remount.
  if (appState.load.kind === "ready" && appState.load.node.fnode === node.fnode) {
    appState.load = { kind: "ready", node };
  }
}

export function canGoBack(): boolean {
  return appState.historyIdx > 0;
}

export function canGoForward(): boolean {
  return appState.historyIdx < appState.history.length - 1;
}

export function goBack() {
  if (!canGoBack()) return;
  window.history.back();
}

export function goForward() {
  if (!canGoForward()) return;
  window.history.forward();
}
