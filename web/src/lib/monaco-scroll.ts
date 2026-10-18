import type { editor as MonacoEditor } from 'monaco-editor';
import { chainEditorScroll } from './editor-scroll';

/** Let the browser scroll; Monaco only renders the corresponding viewport. */
export function nativeMonacoScroll(editor: MonacoEditor.IStandaloneCodeEditor, scroller: HTMLElement) {
  const size = scroller.firstElementChild as HTMLElement;
  const host = editor.getDomNode()!.parentElement!;
  const syncSize = () => {
    size.style.height = `${editor.getScrollHeight()}px`;
    size.style.width = `${editor.getScrollWidth()}px`;
  };
  const layout = () => {
    const {clientWidth: width, clientHeight: height} = scroller;
    if (!width || !height) return;
    host.style.width = `${width}px`;
    host.style.height = `${height}px`;
    editor.layout({width, height});
    syncSize();
  };
  const scroll = () => editor.setScrollPosition({scrollTop: scroller.scrollTop, scrollLeft: scroller.scrollLeft});
  scroller.addEventListener('scroll', scroll);
  const changes = [editor.onDidContentSizeChange(syncSize), editor.onDidScrollChange(event => {
    // Keyboard navigation, selection and restoring a node's saved viewport.
    if (Math.abs(scroller.scrollTop - event.scrollTop) > 1) scroller.scrollTop = event.scrollTop;
    if (Math.abs(scroller.scrollLeft - event.scrollLeft) > 1) scroller.scrollLeft = event.scrollLeft;
  })];
  const resize = new ResizeObserver(layout);
  resize.observe(scroller);
  layout();
  const chain = chainEditorScroll(scroller);
  return () => {
    chain.destroy();
    resize.disconnect();
    scroller.removeEventListener('scroll', scroll);
    changes.forEach(change => change.dispose());
  };
}
