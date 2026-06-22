import type { NodeDetail, NodeInfo } from "./types";
import { api } from "./api";

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
  /** fnode of the previously focused node — highlighted in columns. */
  lastVisitedFnode: null as string | null,
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
function withViewTransition(direction: "up" | "down" | "neutral", mutate: () => void): void {
  if (!supportsViewTransitions()) {
    mutate();
    return;
  }
  document.body.dataset.vtDirection = direction;
  try {
    const vt = (document as Document & {
      startViewTransition: (cb: () => void) => { finished: Promise<void> };
    }).startViewTransition(() => {
      mutate();
    });
    void vt.finished.then(() => {
      delete document.body.dataset.vtDirection;
    }).catch(() => {
      delete document.body.dataset.vtDirection;
    });
  } catch {
    delete document.body.dataset.vtDirection;
    mutate();
  }
}

/** Navigate to a node by fnode. Updates the center, both columns, and history. */
export async function navigate(
  fnode: string,
  opts: { pushHistory?: boolean; direction?: "up" | "down" | "neutral"; skipTransition?: boolean } = {},
) {
  const push = opts.pushHistory ?? true;
  const direction = opts.direction ?? "neutral";
  const skipTransition = opts.skipTransition ?? false;

  // Record where we're leaving from for the last-visited highlight.
  const leaving = appState.load.kind === "ready" ? appState.load.node.fnode : null;
  appState.lastVisitedFnode = leaving;

  // Fetch new node data while keeping the old node visible.
  // The old content stays on screen until the View Transition snapshot
  // is taken (inside withViewTransition's callback), so there's no flash.
  try {
    const [node, refs, kids] = await Promise.all([
      api.node(fnode),
      api.referrers(fnode),
      api.children(fnode),
    ]);

    const apply = () => {
      appState.load = { kind: "ready", node };
      appState.referrers = { items: refs, selected: -1 };
      appState.children = { items: kids, selected: -1 };
      if (push) {
        appState.history = [
          ...appState.history.slice(0, appState.historyIdx + 1),
          fnode,
        ];
        appState.historyIdx = appState.history.length - 1;
      }
    };

    if (skipTransition) {
      apply();
    } else {
      withViewTransition(direction, apply);
    }
  } catch (e) {
    appState.load = {
      kind: "error",
      message: e instanceof Error ? e.message : String(e),
    };
  }
}

/** Refresh only the focused node detail after a write (no view transition). */
export async function refreshFocused(node: NodeDetail) {
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

export async function goBack() {
  if (!canGoBack()) return;
  const target = appState.history[appState.historyIdx - 1]!;
  await navigate(target, { pushHistory: false });
  appState.historyIdx -= 1;
}

export async function goForward() {
  if (!canGoForward()) return;
  const target = appState.history[appState.historyIdx + 1]!;
  await navigate(target, { pushHistory: false });
  appState.historyIdx += 1;
}
