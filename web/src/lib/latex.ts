import { fetchJson } from './api';
import { projectPath } from './project-path';

export interface LatexProject {
  preamble_name: string;
  preamble: string;
  bibliography_name: string;
  bibliography: string;
}
export interface LatexLabel { label: string; name: string; type: string; number: string; anchor: string }
export interface LatexReference extends LatexLabel { key: string; fnode: string; title: string }
export interface LatexImport { fnode: string; title: string; prefix: string }
export interface Citation { key: string; label: string; title: string; authors: string; year: string; text: string }
export interface LatexCatalog { project_key: string; citations: Citation[]; commands: string[]; environments: string[]; diagnostics: string[] }
export interface LatexContext { context_key: string; project_key: string; imports: LatexImport[]; references: LatexReference[]; diagnostics: string[] }
export interface LatexPreviewResult { project_key: string; html: string; labels: LatexLabel[]; diagnostics: string[] }
interface Unchanged { unchanged: true; project_key: string }
const nodePath = (fnode: string) => `/api/node/${encodeURIComponent(fnode)}/latex`;
export const latexApi = {
  project: () => fetchJson<{revision: string; project: LatexProject}>(projectPath('/api/project/latex')),
  putProject: (project: LatexProject, revision: string) => fetchJson(projectPath('/api/project/latex'), {
    method: 'PUT', headers: {'content-type': 'application/json', 'if-match': `"${revision}"`}, body: JSON.stringify(project),
  }),
  catalog: (known = '', signal?: AbortSignal) => fetchJson<LatexCatalog | Unchanged>(projectPath(`/api/project/latex/catalog?known=${encodeURIComponent(known)}`), {signal}),
  context: (fnode: string, known = '', signal?: AbortSignal) => fetchJson<LatexContext | Unchanged>(projectPath(`${nodePath(fnode)}/context?known=${encodeURIComponent(known)}`), {signal}),
  preview: (fnode: string, source: string, signal?: AbortSignal) => fetchJson<LatexPreviewResult>(projectPath(`${nodePath(fnode)}/preview`), {
    method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify({source}), signal,
  }),
};
