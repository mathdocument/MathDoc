import { useLayoutEffect, useRef } from 'react';
/** Native editor callbacks read committed props without restarting their session. */
export function useLatest<T>(value: T) {
  const ref = useRef(value);
  useLayoutEffect(() => { ref.current = value; });
  return ref;
}
