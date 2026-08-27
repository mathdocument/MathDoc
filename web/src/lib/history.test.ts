import { describe, expect, it } from "vitest";
import { browserHistoryEntry, browserHistoryTarget, focusedHistoryState } from "./history";

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

  it("gives a cleared graph selection its own history slot", () => {
    expect(focusedHistoryState(["a", "b"], 1, "b", {})).toEqual({
      entries: ["a", "b", "b"],
      index: 2,
    });
  });
});

describe("browserHistoryEntry", () => {
  it("resolves the backing node for a cleared graph selection", () => {
    const entry = browserHistoryEntry({
      mdcHistory: 1,
      fnode: null,
      index: 2,
      entries: ["a", "b", "b"],
    });

    expect(entry).not.toBeNull();
    expect(browserHistoryTarget(entry!)).toBe("b");
  });

  it("rejects cleared selections without a valid backing slot", () => {
    expect(browserHistoryEntry({
      mdcHistory: 1,
      fnode: null,
      index: 2,
      entries: ["a", "b"],
    })).toBeNull();
  });

  it("still rejects selected nodes that disagree with their history slot", () => {
    expect(browserHistoryEntry({
      mdcHistory: 1,
      fnode: "c",
      index: 1,
      entries: ["a", "b"],
    })).toBeNull();
  });
});
