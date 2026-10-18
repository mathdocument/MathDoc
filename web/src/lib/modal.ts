export function modal(dialog: HTMLDialogElement, initialFocus?: string) {
  const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "Escape" || event.isComposing) return;
    // Search inputs in WebKit consume Escape before the dialog can cancel.
    event.preventDefault();
    event.stopPropagation();
    dialog.requestClose();
  }
  dialog.addEventListener("keydown", onKeyDown, true);
  dialog.showModal();
  if (initialFocus) (dialog.querySelector<HTMLElement>(initialFocus) ?? dialog).focus();
  return { destroy: () => {
    dialog.close();
    dialog.removeEventListener("keydown", onKeyDown, true);
    previousFocus?.focus({ preventScroll: true });
  } };
}
