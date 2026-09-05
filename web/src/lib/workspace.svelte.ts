import { api } from "./api";
import type { GraphCheckReport } from "./types";

/** Owns graph refresh ordering and invalidation independently of mounted views. */
export class WorkspaceSession {
  report = $state<GraphCheckReport | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  stale = $state(false);
  private request = 0;

  get issueCount(): number {
    const report = this.report;
    return report ? report.missing.length + report.invalid.length + report.cycles.length : 0;
  }

  get title(): string {
    if (this.error) return this.error;
    if (this.stale) return "Graph counts updated locally; refresh to recheck issues";
    if (!this.report) return "Checking graph";
    return this.issueCount === 0 ? "Graph check: no issues"
      : `Graph check: ${this.issueCount} issue${this.issueCount === 1 ? "" : "s"}`;
  }

  cancel(): void {
    this.request++;
    this.loading = false;
  }

  async refresh(refreshWorkspace = false): Promise<boolean> {
    const request = ++this.request;
    this.loading = true;
    this.error = null;
    try {
      const report = await (refreshWorkspace ? api.refreshWorkspace() : api.graphCheck());
      if (request !== this.request) return false;
      this.report = report;
      this.stale = false;
      return true;
    } catch (error) {
      if (request === this.request) {
        this.error = error instanceof Error ? error.message : String(error);
      }
      return false;
    } finally {
      if (request === this.request) this.loading = false;
    }
  }

  applyDelta(nodes: number, edges: number): void {
    this.cancel();
    this.error = null;
    if (!this.report) {
      void this.refresh();
      return;
    }
    this.report = {
      ...this.report,
      nodes: Math.max(0, this.report.nodes + nodes),
      edges: Math.max(0, this.report.edges + edges),
    };
    this.stale = true;
  }
}
