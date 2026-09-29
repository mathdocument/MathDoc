// Local Lean sessions and checks of the legacy backend. Only the native Lean editor
// (MDC_WEB_LEAN_EDITOR builds) imports this module, so other builds carry none of it.
import { fetchJson } from "./api";
import { projectPath } from "./project-path";

export const leanApi = {
  leanSession: (fnode: string, revision: string) => fetchJson<{ id: string; filename: string; source: string }>(projectPath(`/api/node/${encodeURIComponent(fnode)}/lean/session`), {
    method: "POST", headers: { "if-match": `"${revision}"` },
  }),
  closeLeanSession: (id: string) => fetchJson<void>(projectPath(`/api/lean/session/${encodeURIComponent(id)}`), {
    method: "DELETE", keepalive: true,
  }),
  checkLean: (fnode: string, revision: string, build = false) => fetchJson<{ fnode: string; revision: string; passed: boolean; certified: boolean; has_sorry: boolean | null; built: boolean; cache_hit: boolean; elapsed_ms: number; diagnostics: unknown[]; dependency_errors: string[] }>(projectPath(`/api/node/${encodeURIComponent(fnode)}/lean/check`), {
    method: "POST", headers: { "content-type": "application/json", "if-match": `"${revision}"` }, body: JSON.stringify({ build }),
  }),
};
