import { tick } from "svelte";

/** Finish editor wrapping/gutters and resize observers before revealing a layout. */
export async function settleEditorLayout(): Promise<void> {
  await tick();
  await document.fonts.ready;
  // Native Monaco scroll viewports resize through ResizeObserver before painting.
  await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
}
