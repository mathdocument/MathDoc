import { describe, expect, it } from "vitest";
import { browserHistoryEntry, focusedHistoryState } from "./history";

describe("focusedHistoryState", () => {
  it("adopts the complete popped entry after a reload", () => {
    const restored = focusedHistoryState(["a", "b"], 1, "c", {
      pushHistory: false,
      historyIndex: 2,
      historyEntries: ["a", "b", "c"],
    });

    expect(restored).toEqual({ entries: ["a", "b", "c"], index: 2 });
    expect(browserHistoryEntry({
      mdcHistory: 1,
      fnode: "c",
      index: restored.index,
      entries: restored.entries,
    })).not.toBeNull();
  });

  it("preserves the forward branch when going back", () => {
    expect(focusedHistoryState(["a", "b", "c"], 2, "b", {
      pushHistory: false,
      historyIndex: 1,
      historyEntries: ["a", "b"],
    })).toEqual({ entries: ["a", "b", "c"], index: 1 });
  });

  it("truncates the forward branch when pushing", () => {
    expect(focusedHistoryState(["a", "b", "c"], 1, "d", {})).toEqual({
      entries: ["a", "b", "d"],
      index: 2,
    });
  });
});
