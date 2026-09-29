import { useLayoutEffect } from "react";

const owners = new WeakMap<object, symbol>();

/** Fast Refresh and Strict Mode reattach effects in the same turn. Dispose only
 * when a native session actually leaves the tree, preserving its source/undo. */
export function useSessionLifetime(session: { destroy(): void }) {
  useLayoutEffect(() => {
    const owner = Symbol();
    owners.set(session, owner);
    return () =>
      queueMicrotask(() => {
        if (owners.get(session) === owner) {
          owners.delete(session);
          session.destroy();
        }
      });
  }, [session]);
}
