import type { BrowserHistoryEntry, BrowserHistoryMode } from "./history";

export interface BrowserHistoryAdapter {
  state(): unknown;
  commit(mode: Exclude<BrowserHistoryMode, "none">, fnode: string | null,
    entry: BrowserHistoryEntry | null): void;
}

export const browserHistoryAdapter: BrowserHistoryAdapter = {
  state: () => window.history.state,
  commit(mode, fnode, entry) {
    const url = new URL(window.location.href);
    url.hash = fnode === null ? "" : new URLSearchParams({ ref: fnode }).toString();
    if (mode === "push") window.history.pushState(entry, "", url);
    else window.history.replaceState(entry, "", url);
  },
};
