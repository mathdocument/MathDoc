export function projectName(pathname = globalThis.location?.pathname ?? "/"): string | null {
  const match = /^\/p\/([A-Za-z0-9_-]+)\/([A-Za-z0-9_-]+)(?:\/|$)/.exec(pathname);
  return match ? `${match[1]}/${match[2]}` : null;
}

export function projectPath(path: string): string {
  const project = projectName();
  return `${project ? `/p/${project}` : ""}${path}`;
}
