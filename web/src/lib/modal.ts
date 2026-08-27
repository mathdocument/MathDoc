export function modal(dialog: HTMLDialogElement) {
  dialog.showModal();
  return {
    destroy() {
      dialog.close();
    },
  };
}
