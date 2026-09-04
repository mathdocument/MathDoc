export type BrowserHistoryMode = "push" | "replace" | "none";

export interface BrowserHistoryEntry {
  mdcHistory: 1;
  fnode: string | null;
  index: number;
  entries: string[];
}

export interface FocusedHistoryOptions {
  pushHistory?: boolean;
  historyIndex?: number;
  historyEntries?: string[];
  browserHistory?: BrowserHistoryMode;
}

export function browserHistoryEntry(value: unknown): BrowserHistoryEntry | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<BrowserHistoryEntry>;
  if (
    candidate.mdcHistory !== 1 ||
    (candidate.fnode !== null && typeof candidate.fnode !== "string") ||
    !Number.isInteger(candidate.index) ||
    !Array.isArray(candidate.entries) ||
    !candidate.entries.every((entry) => typeof entry === "string") ||
    candidate.index! < 0 ||
    candidate.index! >= candidate.entries.length ||
    (candidate.fnode !== null && candidate.entries[candidate.index!] !== candidate.fnode)
  ) {
    return null;
  }
  return candidate as BrowserHistoryEntry;
}

export function browserHistoryTarget(entry: BrowserHistoryEntry): string {
  return entry.fnode ?? entry.entries[entry.index]!;
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
