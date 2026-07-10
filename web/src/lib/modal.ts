const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

function focusableElements(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE))
    .filter((element) => element.getClientRects().length > 0);
}

/** Trap focus inside a modal dialog and restore it when the dialog closes. */
export function modal(dialog: HTMLElement) {
  const previous = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;

  function focusFirst() {
    if (!dialog.contains(document.activeElement)) {
      (focusableElements(dialog)[0] ?? dialog).focus();
    }
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "Tab") return;
    const elements = focusableElements(dialog);
    if (elements.length === 0) {
      event.preventDefault();
      dialog.focus();
      return;
    }

    const first = elements[0]!;
    const last = elements[elements.length - 1]!;
    const current = document.activeElement;
    if (event.shiftKey && (current === first || !dialog.contains(current))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && current === last) {
      event.preventDefault();
      first.focus();
    }
  }

  dialog.addEventListener("keydown", onKeyDown);
  queueMicrotask(focusFirst);

  return {
    destroy() {
      dialog.removeEventListener("keydown", onKeyDown);
      queueMicrotask(() => {
        if (previous?.isConnected) previous.focus();
      });
    },
  };
}
