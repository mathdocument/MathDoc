export function projectName(pathname = globalThis.location?.pathname ?? "/"): string | null {
  const match = /^\/p\/([A-Za-z0-9_-]+)\/([A-Za-z0-9_-]+)(?:\/|$)/.exec(pathname);
  return match ? `${match[1]}/${match[2]}` : null;
}

export function projectPath(path: string): string {
  const project = projectName();
  return `${project ? `/p/${project}` : ""}${path}`;
}

/** The collaboration page of the current branch; `board` selects one of its project boards. */
export function collabPath(board?: string | null): string {
  return projectPath(`/collab${board ? `?board=${encodeURIComponent(board)}` : ""}`);
}

export function isCollabPage(pathname = globalThis.location?.pathname ?? "/"): boolean {
  return /^\/p\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\/collab\/?$/.test(pathname);
}

/** The import page of the current database (docs/external-proofs.md §3.3). */
export function importPath(query = ""): string {
  return projectPath(`/import${query}`);
}

export function isImportPage(pathname = globalThis.location?.pathname ?? "/"): boolean {
  return /^\/p\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\/import\/?$/.test(pathname);
}
