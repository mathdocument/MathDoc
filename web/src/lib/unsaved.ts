const dirtyDrafts = new Set<symbol>();
const pendingMutations = new Set<symbol>();
let pendingWaiters: Array<() => void> = [];
let draftRevision = 0;

export function setDraftDirty(id: symbol, dirty: boolean): void {
  if (dirty) {
    dirtyDrafts.add(id);
    draftRevision++;
  } else if (dirtyDrafts.delete(id)) {
    draftRevision++;
  }
}

export function removeDraft(id: symbol): void {
  if (dirtyDrafts.delete(id)) draftRevision++;
}

export function setMutationPending(id: symbol, pending: boolean): void {
  if (pending) {
    if (!pendingMutations.has(id)) {
      pendingMutations.add(id);
      draftRevision++;
    }
  } else if (pendingMutations.delete(id)) {
    draftRevision++;
    if (pendingMutations.size === 0) {
      const waiters = pendingWaiters;
      pendingWaiters = [];
      for (const resolve of waiters) resolve();
    }
  }
}

export function waitForPendingMutations(): Promise<void> {
  if (pendingMutations.size === 0) return Promise.resolve();
  return new Promise((resolve) => pendingWaiters.push(resolve));
}

export async function settlePendingMutations(): Promise<boolean> {
  const revisionBeforeWait = draftRevision;
  await waitForPendingMutations();
  if (draftRevision !== revisionBeforeWait && hasUnsavedDrafts()) {
    return confirmDiscardDrafts();
  }
  return true;
}

export function hasUnsavedDrafts(): boolean {
  return dirtyDrafts.size > 0;
}

export function unsavedDraftRevision(): number {
  return draftRevision;
}

export function confirmDiscardDrafts(): boolean {
  return !hasUnsavedDrafts() || window.confirm(
    "You have unsaved block or title edits. Discard them?",
  );
}
