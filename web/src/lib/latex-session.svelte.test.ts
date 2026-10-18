import { expect, test, vi } from 'vitest';
import { LatexSession } from './latex-session.svelte';
import { latexApi, type LatexContext, type LatexPreviewResult } from './latex';

test('activation renders immediately or uses the prepared result; only typing is debounced', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('window', {addEventListener: vi.fn(), removeEventListener: vi.fn()});
  const preview: LatexPreviewResult = {project_key: 'p', context_key: 'c', html: 'ready', labels: [], diagnostics: []};
  const session = new LatexSession('node', 'source');
  const prepared = new LatexSession('prepared', 'source', {preview, error: null});
  vi.spyOn(session, 'refresh').mockResolvedValue();
  vi.spyOn(prepared, 'refresh').mockResolvedValue();
  const render = vi.spyOn(session, 'render').mockResolvedValue();
  const preparedRender = vi.spyOn(prepared, 'render').mockResolvedValue();
  try {
    session.setActive(true);
    expect(render).toHaveBeenCalledTimes(1);
    session.schedule('draft');
    await vi.advanceTimersByTimeAsync(249);
    expect(render).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(render).toHaveBeenCalledTimes(2);
    prepared.setActive(true);
    expect(prepared.preview?.html).toBe('ready');
    expect(prepared.previewSource).toBe('source');
    expect(preparedRender).not.toHaveBeenCalled();
    prepared.setActive(false);
    prepared.setActive(true);
    expect(preparedRender).toHaveBeenCalledTimes(1);
  } finally {
    session.destroy(); prepared.destroy();
    vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers();
  }
});

test('superseded previews cannot replace the current draft or its local labels', async () => {
  const requests: Array<(value: LatexPreviewResult) => void> = [];
  const mock = vi.spyOn(latexApi, 'preview').mockImplementation(() => new Promise(resolve => requests.push(resolve)));
  const session = new LatexSession('node', 'old');
  try {
    const first = session.render();
    session.schedule('new');
    const second = session.render();
    const result = (label: string): LatexPreviewResult => ({project_key: 'p', context_key: label, html: label, labels: [{label, name: label, type: 'Section', number: '1', anchor: label}], diagnostics: []});
    requests[1](result('new')); await second;
    requests[0](result('old')); await first;
    expect(session.preview?.html).toBe('new');
    expect(session.references.map(ref => ref.key)).toEqual(['new']);
    expect(session.working).toBe(false);
  } finally { session.destroy(); mock.mockRestore(); }
});

test('context polling leaves a slow request running and rejects a mismatched catalog', async () => {
  let finish!: (value: LatexContext) => void;
  const context = vi.spyOn(latexApi, 'context').mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const catalog = vi.spyOn(latexApi, 'catalog');
  const session = new LatexSession('node', '', {preview: {project_key: 'old', context_key: 'old-context', html: 'old preview', labels: [], diagnostics: []}, error: null});
  const render = vi.spyOn(session, 'render').mockResolvedValue();
  const project = {project_key: 'polling-project', citations: [], commands: ['correct'], environments: [], diagnostics: []};
  const result: LatexContext = {project_key: project.project_key, context_key: 'context', imports: [], references: [], diagnostics: []};
  try {
    const first = session.refresh();
    await session.refresh();
    expect(context).toHaveBeenCalledTimes(1);
    expect(context.mock.calls[0][2]?.aborted).toBe(false);
    catalog.mockResolvedValueOnce({...project, project_key: 'changed-project'});
    finish(result); await first;
    expect(render).toHaveBeenCalledTimes(1);
    expect(session.catalog).toBeNull();
    expect(session.contextError).toContain('project changed');
    context.mockResolvedValue({unchanged: true, project_key: project.project_key});
    catalog.mockResolvedValueOnce(project);
    await session.refresh();
    expect(catalog).toHaveBeenCalledTimes(2);
    expect(session.catalog?.commands).toEqual(['correct']);
    expect(session.contextError).toBeNull();
  } finally { session.destroy(); context.mockRestore(); catalog.mockRestore(); render.mockRestore(); }
});
