import type { editor } from 'monaco-editor';
import {KeyCode, editor as monaco} from 'monaco-editor';
import { latexCompletions } from './latex-completion';
import type { LatexSession } from './latex-session.svelte';

/** Small completion lists use native scrolling, without recycled rows or wheel handlers. */
export function latexAutocomplete(session: LatexSession, target: editor.IStandaloneCodeEditor) {
  target.updateOptions({quickSuggestions: false, suggestOnTriggerCharacters: false, wordBasedSuggestions: 'off'});
  const list = document.createElement('div');
  list.className = 'latex-completions';
  list.id = `latex-completions-${target.getId()}`;
  list.setAttribute('role', 'listbox');
  list.setAttribute('aria-label', 'LaTeX suggestions');
  // The portalled popup lives outside Monaco's focus handler.
  list.onmousedown = event => event.preventDefault();
  const input = target.getDomNode()?.querySelector('textarea');
  let result: ReturnType<typeof latexCompletions> = null;
  let anchor: ReturnType<typeof target.getPosition> = null;
  let range: editor.IIdentifiedSingleEditOperation['range'];
  let selected = 0, accepting = false, disposed = false, pending = false;
  const widget: editor.IContentWidget = {
    getId: () => list.id, getDomNode: () => list,
    allowEditorOverflow: true, suppressMouseDown: true,
    getPosition: () => anchor ? {position: anchor, preference: [monaco.ContentWidgetPositionPreference.BELOW, monaco.ContentWidgetPositionPreference.ABOVE]} : null,
  };
  target.addContentWidget(widget);
  const hide = () => {
    anchor = null; result = null;
    input?.removeAttribute('aria-activedescendant');
    input?.removeAttribute('aria-controls');
    target.layoutContentWidget(widget);
  };
  const select = (index: number, reveal = false) => {
    const next = Math.max(0, Math.min(list.children.length - 1, index));
    if (next === selected) return;
    list.children[selected]?.setAttribute('aria-selected', 'false');
    selected = next;
    const row = list.children[selected] as HTMLElement;
    row.setAttribute('aria-selected', 'true');
    input?.setAttribute('aria-activedescendant', row.id);
    if (reveal) {
      // Scroll only this popup; scrollIntoView can also move the editor/page.
      if (row.offsetTop < list.scrollTop) list.scrollTop = row.offsetTop;
      else if (row.offsetTop + row.offsetHeight > list.scrollTop + list.clientHeight)
        list.scrollTop = row.offsetTop + row.offsetHeight - list.clientHeight;
    }
  };
  const refresh = () => {
    if (disposed || accepting || !target.hasTextFocus()) return;
    const model = target.getModel(), position = target.getPosition();
    if (!model || !position || !target.getSelection()?.isEmpty()) { hide(); return; }
    const offset = model.getOffsetAt(position);
    result = latexCompletions(session, model.getValue().slice(Math.max(0, offset - 1024), offset));
    if (!result?.options.length) { hide(); return; }
    anchor = model.getPositionAt(offset - result.length);
    range = {startLineNumber: anchor.lineNumber, startColumn: anchor.column, endLineNumber: position.lineNumber, endColumn: position.column};
    list.replaceChildren(...result.options.map((item, index) => {
      const row = document.createElement('div');
      row.id = `${list.id}-${index}`; row.dataset.index = String(index);
      row.setAttribute('role', 'option'); row.title = [item.label, item.detail].filter(Boolean).join(' · ');
      row.setAttribute('aria-selected', 'false');
      const label = document.createElement('span'), detail = document.createElement('span');
      label.textContent = item.label; detail.textContent = item.detail;
      row.append(label, detail);
      return row;
    }));
    list.scrollTop = 0;
    input?.setAttribute('aria-controls', list.id);
    selected = -1;
    select(0);
    target.layoutContentWidget(widget);
  };
  const schedule = () => {
    if (pending || accepting) return;
    pending = true;
    queueMicrotask(() => { pending = false; refresh(); });
  };
  const accept = (index: number) => {
    const item = result?.options[index];
    if (!item || !anchor) return;
    accepting = true;
    const model = target.getModel()!, start = model.getOffsetAt(anchor);
    target.pushUndoStop();
    target.executeEdits('latex-completion', [{range, text: item.insert, forceMoveMarkers: true}]);
    target.setPosition(model.getPositionAt(start + item.insert.length));
    target.pushUndoStop();
    hide(); target.focus(); accepting = false;
  };
  list.onclick = event => {
    const row = (event.target as Element).closest<HTMLElement>('[role="option"]');
    if (row) accept(Number(row.dataset.index));
  };
  list.onpointermove = event => {
    const row = (event.target as Element).closest<HTMLElement>('[role="option"]');
    if (row) select(Number(row.dataset.index));
  };
  const subscriptions = [
    target.onDidChangeModelContent(schedule),
    target.onDidChangeCursorPosition(() => { if (anchor) schedule(); }),
    target.onDidBlurEditorText(hide),
    target.onDidScrollChange(hide),
    target.onKeyDown(event => {
      if (event.ctrlKey && event.keyCode === KeyCode.Space) {
        event.preventDefault(); event.stopPropagation(); refresh(); return;
      }
      if (!anchor) return;
      switch (event.keyCode) {
        case KeyCode.DownArrow: select(selected + 1, true); break;
        case KeyCode.UpArrow: select(selected - 1, true); break;
        case KeyCode.PageDown: select(selected + 10, true); break;
        case KeyCode.PageUp: select(selected - 10, true); break;
        case KeyCode.Enter: case KeyCode.Tab: accept(selected); break;
        case KeyCode.Escape: hide(); break;
        default: return;
      }
      event.preventDefault(); event.stopPropagation();
    }),
  ];
  return {dispose() {
    disposed = true; subscriptions.forEach(item => item.dispose());
    hide(); target.removeContentWidget(widget); list.remove();
  }};
}
