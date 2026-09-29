import { api, isAbortError } from "../../lib/api";
import type { GraphFull } from "../../lib/types";
import { errMsg } from "../../lib/format";
import { ObservableModel } from "../../lib/observable";

/** Fetch ordering and cached topology; independent of canvas and component lifetimes. */
export class GraphSession extends ObservableModel {
  data: GraphFull | null = null;
  loading = false;
  error: string | null = null;
  private revision = 0;
  private loadedRevision: number | null = null;
  private request: AbortController | null = null;
  private pending: Promise<void> | null = null;
  constructor() {
    super();
    this.observe("data", "loading", "error");
  }
  update(active: boolean, revision: number) {
    if (revision !== this.revision) this.cancel();
    this.revision = revision;
    if (active) void this.prepare();
    else this.cancel();
  }
  prepare(): Promise<void> {
    if (this.pending) return this.pending;
    if (this.loadedRevision === this.revision) return Promise.resolve();
    const revision = this.revision;
    const request = (this.request = new AbortController());
    this.loading = true;
    this.error = null;
    this.pending = api
      .full(request.signal)
      .then((data) => {
        if (request.signal.aborted || this.request !== request) return;
        this.data = data;
        this.loadedRevision = revision;
      })
      .catch((error) => {
        if (!request.signal.aborted && !isAbortError(error))
          this.error = errMsg(error);
      })
      .finally(() => {
        if (this.request !== request) return;
        this.loading = false;
        this.pending = null;
        this.request = null;
      });
    return this.pending;
  }
  cancel() {
    this.request?.abort();
    this.request = null;
    this.pending = null;
    this.loading = false;
  }
}
