// External proof imports (docs/external-proofs.md §3.3): the record, its project checks and the
// proof's source, read from /api/imports. Checks run in LeanGround with Comparator.
import { fetchJson } from "./api";

export interface ImportRecord {
  database: string;
  source: string;
  url?: string;
  commit: string;
  config_name: string;
  config: {
    challenge_module: string;
    solution_module: string;
    theorem_names: string[];
    definition_names: string[];
    permitted_axioms: string[];
  };
  scope: { path: string; family: string; title: string; result: string | null };
  papers: { title: string; path: string; present: boolean }[];
  statement: { module: string; path: string; lines: number };
  proof: { module: string; path: string; files: number; lines: number; external: string[] };
  nodes: { overview: string; papers: string[]; scope: string; statement: string; proof: string };
  created_by: string;
  created_at: string;
}

export type CheckStatus = "submitting" | "queued" | "running" | "passed" | "failed" | "error" | "submit_failed";

export interface CheckTarget {
  name: string;
  kind: "theorem" | "definition";
  status: string;
  /** Definition holes: what the statement module gives, and the second comparison. */
  challenge?: "sorry" | "body" | "not_definition" | "missing" | "unknown" | null;
  comparison?: "identical" | "differs" | "unchecked" | "not_applicable" | null;
  needs_review?: boolean | null;
}

export interface CheckResult {
  workspace?: string;
  isolated?: boolean;
  identity?: {
    commit: string | null;
    dirty: boolean | null;
    lake_manifest_sha256: string | null;
    lean_toolchain: string | null;
    challenge_sha256: string | null;
    solution_sha256: string | null;
    tools: Record<string, string | null>;
  };
  passes?: { definition_names: string[]; status: string; reason: string | null; message?: string | null; seconds: number }[];
  targets?: CheckTarget[];
  reason?: string | null;
  message?: string | null;
  log_tail?: string;
  hole_probe_error?: string;
}

export interface ImportCheck {
  id: string;
  idempotency_key: string;
  status: CheckStatus;
  leanground_check?: string;
  submitted_by: string;
  submitted_at: string;
  completed_at?: string;
  error?: string;
  result?: CheckResult;
}

export interface SourceModule { module: string; path: string; lines: number; imports: string[] }
export interface SourceTree {
  commit: string;
  url: string | null;
  lean: string;
  statement: { module: string; path: string; lines: number };
  root: string;
  modules: SourceModule[];
  external: string[];
}
export interface SourceFile { module: string; path: string; commit: string; url: string | null; text: string }

const base = (db: string) => `/api/imports/${encodeURIComponent(db)}`;
export const importsApi = {
  get: (db: string) => fetchJson<{ import: ImportRecord; checks: ImportCheck[] }>(base(db)),
  check: (db: string) => fetchJson<ImportCheck>(`${base(db)}/checks`, { method: "POST" }),
  source: (db: string) => fetchJson<SourceTree>(`${base(db)}/source`),
  file: (db: string, module: string) =>
    fetchJson<SourceFile>(`${base(db)}/source/file?module=${encodeURIComponent(module)}`),
};

export const inProgress = (s: CheckStatus) => s === "submitting" || s === "queued" || s === "running";

/** How a finished check reads: passed outright, passed with definitions to review, or not. */
export function verdict(c: ImportCheck | undefined): { tone: "ok" | "review" | "bad" | "pending" | "none"; label: string } {
  if (!c) return { tone: "none", label: "Not checked" };
  if (inProgress(c.status)) return { tone: "pending", label: c.status === "running" ? "Checking" : "Queued" };
  if (c.status === "submit_failed") return { tone: "bad", label: "Not submitted" };
  if (c.status === "error") return { tone: "bad", label: "Check error" };
  if (c.status === "failed") return { tone: "bad", label: "Rejected" };
  const review = c.result?.targets?.some((t) => t.needs_review);
  return review ? { tone: "review", label: "Passed · definitions to review" } : { tone: "ok", label: "Passed" };
}
