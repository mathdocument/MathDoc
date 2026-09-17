/** Database module names are validated by Rust; quoted dots belong to a filename. */
export function leanSourcePath(module: string) {
  const parts = module.match(/«[^»]+»|[^.]+/g);
  if (!parts) throw new Error('Missing Lean module name');
  return `/project/${parts.map(part => part.startsWith('«') ? part.slice(1, -1) : part).join('/')}.lean`;
}
