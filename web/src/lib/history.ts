export type BrowserHistoryMode = "push" | "replace" | "none";

export interface BrowserHistoryEntry {
  mdcHistory: 1;
  fnode: string;
  index: number;
  entries: string[];
}

export interface FocusedHistoryOptions {
  pushHistory?: boolean;
  historyIndex?: number;
  historyEntries?: string[];
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

export function focusedHistoryState(
  entries: string[],
  index: number,
  fnode: string,
  opts: FocusedHistoryOptions,
): { entries: string[]; index: number } {
  if (opts.pushHistory ?? true) {
    const nextEntries = [...entries.slice(0, index + 1), fnode];
    return { entries: nextEntries, index: nextEntries.length - 1 };
  }
  const restoredIndex = opts.historyIndex ?? index;
  const currentBranchContainsTarget = entries[restoredIndex] === fnode;
  return {
    entries: !currentBranchContainsTarget && opts.historyEntries
      ? [...opts.historyEntries]
      : [...entries],
    index: restoredIndex,
  };
}
