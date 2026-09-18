import { isAbortError } from './api';
import { errMsg } from './format';
import { latexApi, type LatexCatalog, type LatexContext, type LatexPreviewResult, type LatexReference, type PreparedLatexPreview } from './latex';

const catalogs = new Map<string, Promise<LatexCatalog>>();
function catalog(key: string): Promise<LatexCatalog> {
  let pending = catalogs.get(key);
  if (!pending) {
    pending = latexApi.catalog().then(result => {
      if ('unchanged' in result) throw new Error('Missing LaTeX project catalog');
      if (result.project_key !== key) throw new Error('LaTeX project changed; retry loading completions');
      return result;
    });
    catalogs.set(key, pending);
    void pending.catch(() => { if (catalogs.get(key) === pending) catalogs.delete(key); });
    if (catalogs.size > 4) catalogs.delete(catalogs.keys().next().value!);
  }
  return pending;
}

/** One editor's draft and cancellation state; only immutable project catalogs are shared. */
export class LatexSession {
  context = $state<LatexContext | null>(null);
  catalog = $state<LatexCatalog | null>(null);
  preview = $state<LatexPreviewResult | null>(null);
  source = $state('');
  previewSource = $state<string | null>(null);
  working = $state(false);
  error = $state<string | null>(null);
  contextError = $state<string | null>(null);
  private live = true;
  private active = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private poll: ReturnType<typeof setInterval> | null = null;
  private rendering: AbortController | null = null;
  private loading: AbortController | null = null;
  private changed = () => { void this.refresh(); };

  constructor(readonly fnode: string, source: string, private prepared?: PreparedLatexPreview) {
    this.source = source;
    if (prepared) {
      this.preview = prepared.preview;
      this.error = prepared.error;
      if (prepared.preview) this.previewSource = source;
    }
  }

  get references(): LatexReference[] {
    const external = this.context?.references.filter(ref => ref.fnode !== this.fnode) ?? [];
    const local = this.previewSource === this.source ? this.preview?.labels.map(label => ({ ...label, key: label.label, fnode: this.fnode, title: 'Current node' })) ?? []
      : this.previewSource === null ? this.context?.references.filter(ref => ref.fnode === this.fnode) ?? [] : [];
    return [...local, ...external];
  }

  setActive(active: boolean) {
    if (active === this.active) return;
    this.active = active;
    if (active) {
      // Debounce typing, not navigation. A prepared preview is consumed once.
      if (this.prepared) this.prepared = undefined;
      else void this.render();
      void this.refresh();
      this.poll = setInterval(() => { if (!document.hidden) void this.refresh(); }, 5000);
      window.addEventListener('mdc-latex-project-changed', this.changed);
    } else {
      if (this.poll) clearInterval(this.poll);
      this.poll = null;
      window.removeEventListener('mdc-latex-project-changed', this.changed);
      this.loading?.abort();
      this.loading = null;
      this.rendering?.abort();
      if (this.timer) clearTimeout(this.timer);
      this.working = false;
    }
  }

  async refresh() {
    if (this.loading) return;
    const request = this.loading = new AbortController();
    try {
      const result = await latexApi.context(this.fnode, this.context?.context_key, request.signal);
      if (!this.live || request.signal.aborted) return;
      if (!('unchanged' in result)) {
        const previous = this.context?.context_key ?? this.preview?.context_key;
        this.context = result;
        if (previous && previous !== result.context_key) void this.render();
      }
      if (this.catalog?.project_key !== result.project_key) {
        const next = await catalog(result.project_key);
        if (!this.live || request.signal.aborted) return;
        this.catalog = next;
      }
      this.contextError = null;
    } catch (error) {
      if (this.live && !request.signal.aborted && !isAbortError(error)) this.contextError = errMsg(error);
    } finally {
      if (this.loading === request) this.loading = null;
    }
  }

  schedule(source: string) {
    this.source = source;
    if (this.timer) clearTimeout(this.timer);
    this.rendering?.abort();
    if (!this.active) return;
    this.timer = setTimeout(() => { this.timer = null; void this.render(); }, 250);
  }

  async render() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.rendering?.abort();
    const request = this.rendering = new AbortController();
    const source = this.source;
    this.working = true;
    try {
      const result = await latexApi.preview(this.fnode, source, request.signal);
      if (!this.live || request.signal.aborted) return;
      this.preview = result;
      this.previewSource = source;
      this.error = null;
    } catch (error) {
      if (this.live && !request.signal.aborted && !isAbortError(error)) this.error = errMsg(error);
    } finally {
      if (this.live && !request.signal.aborted) this.working = false;
    }
  }

  destroy() {
    this.setActive(false);
    this.live = false;
    this.loading?.abort();
    this.rendering?.abort();
  }
}
