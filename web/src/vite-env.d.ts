/// <reference types="svelte" />
/// <reference types="vite/client" />

// threads 1.7 ships types, but its exports map omits the type entry.
declare module 'threads' {
  export function spawn<T>(worker: Worker, options?: {timeout?: number}): Promise<T>;
}
