/**
 * Apply a state mutation through the View Transitions API when available,
 * otherwise run it synchronously. The callback must perform all reactive
 * updates that should be part of the transition.
 */
let viewTransitionToken = 0;
let activeViewTransition: ViewTransition | null = null;

export async function withViewTransition(
  mutate: () => void,
  scope?: string,
  ready?: () => Promise<void>,
): Promise<void> {
  if (
    typeof document === "undefined" ||
    typeof document.startViewTransition !== "function"
  ) {
    mutate();
    await ready?.();
    return;
  }
  activeViewTransition?.skipTransition();
  const token = ++viewTransitionToken;
  let applied = false;
  const apply = () => {
    if (applied) return;
    applied = true;
    mutate();
  };
  const cleanup = () => {
    if (token !== viewTransitionToken) return;
    activeViewTransition = null;
    delete document.documentElement.dataset.vtScope;
  };
  if (scope) document.documentElement.dataset.vtScope = scope;
  else delete document.documentElement.dataset.vtScope;
  try {
    const vt = document.startViewTransition(apply);
    activeViewTransition = vt;
    // Superseded/hidden transitions reject ready even though the update succeeds.
    void vt.ready.catch(() => {});
    void vt.finished.then(cleanup).catch(cleanup);
    await vt.updateCallbackDone;
    if (ready) {
      await ready();
      vt.skipTransition();
      await vt.finished;
    }
  } catch {
    if (token === viewTransitionToken) activeViewTransition?.skipTransition();
    cleanup();
    apply();
  }
}
