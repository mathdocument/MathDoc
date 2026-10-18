/// <reference types="svelte" />
/// <reference types="vite/client" />

// threads 1.7 ships types, but its exports map omits the type entry.
declare module 'threads' {
  export function spawn<T>(worker: Worker, options?: {timeout?: number}): Promise<T>;
}

// The pinned Monaco/VS Code package exports these functions without their types.
declare module 'vscode/vscode/vs/base/common/filters' {
  export function fuzzyScore(pattern: string, patternLow: string, patternStart: number,
    word: string, wordLow: string, wordStart: number): number[] | undefined;
  export const fuzzyScoreGracefulAggressive: typeof fuzzyScore;
}
