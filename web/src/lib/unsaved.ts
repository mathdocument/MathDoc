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

function setMutationPending(id: symbol, pending: boolean): void {
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

export function trackMutation(): () => void {
  const id = Symbol("pending mutation");
  setMutationPending(id, true);
  return () => setMutationPending(id, false);
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

export function hasUnsavedDrafts(excludeDraft?: symbol): boolean {
  const hasDirtyDraft = excludeDraft === undefined
    ? dirtyDrafts.size > 0
    : dirtyDrafts.size > (dirtyDrafts.has(excludeDraft) ? 1 : 0);
  return hasDirtyDraft || pendingMutations.size > 0;
}

export function unsavedDraftRevision(): number {
  return draftRevision;
}

export function confirmDiscardDrafts(excludeDraft?: symbol): boolean {
  return !hasUnsavedDrafts(excludeDraft) || window.confirm(
    "You have unsaved edits or pending changes. Discard them?",
  );
}

export function confirmDiscardDraft(id: symbol): boolean {
  return !dirtyDrafts.has(id) || window.confirm("Discard this unfinished draft?");
}
