import { tick } from "svelte";

/** Finish editor wrapping/gutters and resize observers before revealing a layout. */
export async function settleEditorLayout(): Promise<void> {
  await tick();
  await document.fonts.ready;
  const hosts = [...document.querySelectorAll<HTMLElement>(".cm-editor")]
    .filter(host => host.getClientRects().length > 0);
  if (hosts.length) {
    const { EditorView } = await import("@codemirror/view");
    await Promise.all(hosts.map(host => new Promise<void>(resolve => {
      const editor = EditorView.findFromDOM(host);
      if (!editor) return resolve();
      editor.requestMeasure({ read: () => null, write: () => resolve() });
    })));
  }
  // ResizeObserver and Svelte's size bindings can schedule another frame.
  await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
}
