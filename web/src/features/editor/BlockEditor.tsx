import { useLayoutEffect, useRef, useState } from 'react';
import { AlertTriangle, ChevronDown, ChevronRight, Code2, Eye, Trash2 } from 'lucide-react';
import type { LatexImport } from '../../lib/latex';
import { useModel } from '../../hooks/use-model';
import { SourceEditorSession, type SourceEditorSessionProps } from './source-session';
import { LatexPreview } from './LatexPreview';

function LatexImports({ imports }: { imports: LatexImport[] | null }) {
  const [query, setQuery] = useState('');
  const matches = imports?.filter(item => `${item.name} ${item.fnode}`.toLowerCase().includes(query.toLowerCase())) ?? [];
  return <details className="latex-imports"><summary>{imports ? `${imports.length} imported dependencies` : 'Loading reference scope...'}</summary><div className="imports-body">{(imports?.length ?? 0) > 20 && <input aria-label="Filter LaTeX imports" placeholder="Filter dependencies..." value={query} onChange={event => setQuery(event.target.value)} />}<ul>{matches.slice(0, 50).map(item => <li title={`${item.fnode} ${item.name}`} key={item.fnode}><span>{item.name}</span></li>)}</ul>{matches.length > 50 && <p>Showing 50 of {matches.length}. Refine the filter to see more.</p>}</div></details>;
}
export default function BlockEditor(props: SourceEditorSessionProps) {
  const [session] = useState(() => new SourceEditorSession(props));
  const s = useModel(session);
  const host = useRef<HTMLDivElement>(null), scroller = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { s.attach(host.current!, scroller.current!); return () => s.destroy(); }, [s]);
  useLayoutEffect(() => { s.update(props); });
  return <article className="source-block" data-srctype={props.block.srctype}>
    <header className="block-head"><span className="srctype">{props.block.srctype}</span><span className="spacer" />
      {s.dirty && <span className="dirty" title="Unsaved changes"><span className="dirty-dot" /><span className="btn-label">Unsaved</span></span>}
      {s.latex?.working && <span className="saving">rendering...</span>}{s.deleting && <span className="saving">deleting...</span>}{s.error && <span className="error" title={s.error}><AlertTriangle size={14} /></span>}
      {props.block.srctype === 'latex' && <button className={`preview-toggle ${s.previewing ? 'active' : ''}`} onClick={() => props.onPreviewChange(!props.latexPreview)} disabled={!s.expanded} aria-pressed={s.previewing} aria-label={s.previewing ? 'Return to LaTeX editor' : 'Render LaTeX preview'} title={s.previewing ? 'Return to LaTeX editor' : 'Render LaTeX preview'}>{s.previewing ? <><Code2 size={14} /><span className="btn-label">Edit</span></> : <><Eye size={14} /><span className="btn-label">Preview</span></>}</button>}
      <button className="icon-btn expand" onClick={s.toggleExpand} aria-label={s.expanded ? 'Collapse block' : 'Expand block'} title={s.expanded ? 'Collapse' : 'Expand'}>{s.expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}</button>
      <button className="delete" onClick={() => void s.onDelete()} disabled={props.saving || s.deleting} aria-label="Delete block" title="Delete block"><Trash2 size={14} /></button>
    </header>
    {s.latex && s.expanded && <LatexImports imports={s.latex.context?.imports ?? null} />}
    <div className={`editor-scroll ${!s.ready ? 'pending' : ''} ${!s.expanded || s.showPreview ? 'collapsed' : ''}`} inert={s.deleting || !s.ready || !s.expanded || s.showPreview} ref={scroller}><div className="editor-size"><div className="editor-host" ref={host} /></div></div>
    {s.showPreview && s.expanded && (s.latex?.preview ? <LatexPreview html={s.latex.preview.html} labels={s.latex.preview.labels} fnode={props.fnode} focusLabel={props.focusLabel} onNavigate={props.onLatexNavigate} /> : !s.latex?.error && <div className="preview-loading" aria-busy="true">Preparing preview...</div>)}
    {(s.latex?.error || s.latex?.contextError) && <div className="error-bar" role="alert">{s.latex.error ?? s.latex.contextError}<button onClick={() => { void s.latex?.refresh(); void s.latex?.render(); }}>Retry</button></div>}
    {!!s.latex?.preview?.diagnostics.length && <details className="latex-diagnostics"><summary>{s.latex.preview.diagnostics.length} LaTeX diagnostic(s)</summary>{s.latex.preview.diagnostics.map((message, i) => <p key={i}>{message}</p>)}</details>}
    {s.error && <div className="error-bar">{s.error}</div>}
  </article>;
}
