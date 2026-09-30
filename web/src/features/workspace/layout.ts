import type { Layout } from "react-resizable-panels";
export type WorkspaceView = "columns" | "force";
export function readLayout(view: WorkspaceView): Layout {
  if (view === "columns") return { context: 22, editor: 56, dependencies: 22 };
  const fallback = { context: 0, editor: 40, dependencies: 60 };
  try {
    const value = JSON.parse(
      localStorage.getItem(`mdc-layout-${view}`) ?? "null",
    );
    if (
      value &&
      Object.keys(fallback).every(
        (key) =>
          typeof value[key] === "number" &&
          Number.isFinite(value[key]) &&
          value[key] >= 0 &&
          value[key] <= 100,
      ) &&
      Math.abs(value.context + value.editor + value.dependencies - 100) < 0.1
    )
      return { context: 0, editor: value.editor, dependencies: 100 - value.editor };
  } catch {
    /* Storage may be disabled in a private browser. */
  }
  return fallback;
}
export function saveLayout(view: WorkspaceView, layout: Layout) {
  try {
    localStorage.setItem(`mdc-layout-${view}`, JSON.stringify(layout));
  } catch {
    /* Keep the current layout usable without storage. */
  }
}
