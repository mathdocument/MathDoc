import { expect, test, vi } from 'vitest';
import { LatexSession } from './latex-session.svelte';
import { latexApi, type LatexPreviewResult } from './latex';

test('superseded previews cannot replace the current draft or its local labels', async () => {
  const requests: Array<(value: LatexPreviewResult) => void> = [];
  const mock = vi.spyOn(latexApi, 'preview').mockImplementation(() => new Promise(resolve => requests.push(resolve)));
  const session = new LatexSession('node', 'old');
  try {
    const first = session.render();
    session.schedule('new');
    const second = session.render();
    const result = (label: string): LatexPreviewResult => ({project_key: 'p', html: label, labels: [{label, name: label, type: 'Section', number: '1', anchor: label}], diagnostics: []});
    requests[1](result('new')); await second;
    requests[0](result('old')); await first;
    expect(session.preview?.html).toBe('new');
    expect(session.references.map(ref => ref.key)).toEqual(['new']);
    expect(session.working).toBe(false);
  } finally { session.destroy(); mock.mockRestore(); }
});
