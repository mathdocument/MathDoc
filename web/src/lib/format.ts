export function shortFnode(fnode: string): string {
  return fnode.slice(0, 8);
}

export function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
