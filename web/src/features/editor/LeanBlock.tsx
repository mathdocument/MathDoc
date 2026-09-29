import { useEffect, useLayoutEffect, useState } from 'react';
import { ChevronDown, ChevronRight, Play, RotateCcw, Square, Trash2 } from 'lucide-react';
import { leanImportName } from '../../lib/node-name';
import { projectPath } from '../../lib/project-path';
import { useModel } from '../../hooks/use-model';
import { LeanEditorSession, type LeanEditorSessionProps } from './lean-session';

/** This component stays mounted across node and workspace-view changes. */
export function LeanBlock(props: LeanEditorSessionProps) {
  const [session] = useState(() => new LeanEditorSession(props));
  const s = useModel(session);
  useLayoutEffect(() => { s.update(props); });
  useEffect(() => {
    window.addEventListener('message', s.message);
    return () => { window.removeEventListener('message', s.message); s.destroy(); };
  }, [s]);
  const { block, module } = props;
  return <article className={`source-block lean-block ${!block ? 'hidden' : ''} ${!s.ready && !s.error && !s.opening && !s.session ? 'preparing' : ''}`} data-srctype="lean" data-session={s.session}>
    <header className="block-head"><span className="srctype">lean</span><span className="spacer" />
      {s.dirty && <span className="dirty" title="Unsaved changes"><span className="dirty-dot" /><span className="btn-label">Unsaved</span></span>}
      <div className="block-actions">
        <button className="icon-btn expand" onClick={s.refresh} disabled={s.opening || s.stopping || s.busy || !s.runtimeReady} aria-label={s.session ? 'Recheck Lean' : 'Start Lean server'} title={s.session ? 'Recheck current Lean file' : 'Start Lean server'}>{s.session ? <RotateCcw size={14} /> : <Play size={14} />}</button>
        <button className="icon-btn expand" onClick={() => void s.stop()} disabled={s.stopping || (!s.session && !s.opening)} aria-label="Stop Lean server" title="Stop this page’s Lean server"><Square size={14} /></button>
        <button className="icon-btn expand" onClick={() => { s.expanded = !s.expanded; }} aria-expanded={s.expanded} aria-label={s.expanded ? 'Collapse block' : 'Expand block'} title={s.expanded ? 'Collapse' : 'Expand'}>{s.expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}</button>
        <button className="delete" onClick={() => void s.remove()} disabled={s.busy} aria-label="Delete block" title="Delete block"><Trash2 size={14} /></button>
      </div>
    </header>
    {module && <details className="module-import"><summary>Lean import</summary><code>import {leanImportName(module)}</code></details>}
    {s.deleting && <div className="status activity" role="status">Deleting...</div>}
    {s.validating && !s.dirty && <div className="status activity" role="status">Verifying saved version...</div>}
    {s.ready && s.progress && <div className="status" role="status">{s.progress}</div>}
    <div className={`editor-surface ${!s.expanded ? 'collapsed' : ''}`}>
      {s.mounted && <div className={`native-editor ${!s.ready ? 'pending' : ''}`} inert={!s.ready || !s.expanded}><iframe ref={s.attachFrame} title="Lean source and Infoview" src={projectPath(`/lean.html?theme=${s.initialTheme}`)} allow="clipboard-write" /></div>}
    </div>
    {s.error && <div className="error-bar" role="alert">{s.error}</div>}
    {s.result && <div className={`status ${!s.result.certified ? 'error-bar' : ''}`}>{s.result.certified ? 'Checked' : s.result.passed ? 'Dependency check failed' : 'Lean errors'} {s.result.has_sorry ? '· contains sorry' : ''} {s.result.built ? '· olean ready' : ''} · {s.result.cache_hit ? 'cached' : `${s.result.elapsed_ms} ms`}{s.result.dependency_errors.map((issue, i) => <div key={i}>{issue}</div>)}</div>}
  </article>;
}
