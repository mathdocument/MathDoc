// Markdown editing on Monaco, shared by text blocks and the short texts of the collaboration
// page (notes, review comments): toolbar commands as single undoable edits, their shortcuts,
// and a small auto-growing editor for short texts. Loaded on demand with Monaco.
import { editor as Monaco, KeyCode, KeyMod, Range, Uri } from "monaco-editor";
import { loadSourceLanguage, setMonacoTheme, sourceOptions } from "./monaco";
import { markdownEdit, type MarkdownCommand } from "./markdown-commands";
import { currentTheme } from "./theme";

/** Apply a toolbar command to the editor's selection, as one step of its undo history. */
export function applyMarkdownCommand(view: Monaco.ICodeEditor, command: MarkdownCommand) {
  const model = view.getModel();
  const selection = view.getSelection();
  if (!model || !selection) return;
  const from = model.getOffsetAt(selection.getStartPosition());
  const to = model.getOffsetAt(selection.getEndPosition());
  const e = markdownEdit(model.getValue(), from, to, command);
  const at = (offset: number) => model.getPositionAt(offset);
  view.pushUndoStop();
  view.executeEdits("markdown-toolbar", [{ range: Range.fromPositions(at(e.from), at(e.to)), text: e.insert, forceMoveMarkers: true }]);
  view.setSelection(Range.fromPositions(at(e.selection[0]), at(e.selection[1])));
  view.pushUndoStop();
  view.focus();
}

/** ⌘B and ⌘I, as in other Markdown editors. */
export function bindMarkdownKeys(view: Monaco.IStandaloneCodeEditor) {
  view.addCommand(KeyMod.CtrlCmd | KeyCode.KeyB, () => applyMarkdownCommand(view, "bold"));
  view.addCommand(KeyMod.CtrlCmd | KeyCode.KeyI, () => applyMarkdownCommand(view, "italic"));
}

// Monaco's theme is global: follow the page's, however the page changes it.
let themeWatch: MutationObserver | undefined;
function followTheme() {
  if (themeWatch) return;
  themeWatch = new MutationObserver(() => void setMonacoTheme(currentTheme()));
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
}

export interface MarkdownEditorHandle {
  getValue(): string;
  setValue(value: string): void;
  apply(command: MarkdownCommand): void;
  focus(): void;
  dispose(): void;
}

let models = 0;
/**
 * A Monaco Markdown editor for a short text: no line numbers, as tall as its text between
 * `minHeight` and `maxHeight` (then it scrolls), ⌘↵ to submit and Esc to cancel.
 */
export async function createMarkdownEditor(host: HTMLElement, options: {
  value: string; label: string; placeholder?: string; minHeight?: number; maxHeight?: number;
  onChange(value: string): void; onSubmit?(): void; onCancel?(): void;
}): Promise<MarkdownEditorHandle> {
  const language = await loadSourceLanguage("text");
  await setMonacoTheme(currentTheme());
  followTheme();
  const model = Monaco.createModel(options.value, language, Uri.parse(`inmemory://mdc/markdown/${++models}.md`));
  const view = Monaco.create(host, {
    ...sourceOptions,
    model, ariaLabel: options.label, placeholder: options.placeholder,
    automaticLayout: true, lineNumbers: "off", glyphMargin: false, lineDecorationsWidth: 8,
    renderLineHighlight: "none", overviewRulerLanes: 0, hideCursorInOverviewRuler: true,
    padding: { top: 8, bottom: 8 }, fixedOverflowWidgets: true,
    scrollbar: { vertical: "auto", horizontal: "hidden", handleMouseWheel: true, alwaysConsumeMouseWheel: false, verticalScrollbarSize: 8 },
  });
  const min = options.minHeight ?? 96, max = options.maxHeight ?? 360;
  const fit = () => { host.style.height = `${Math.min(max, Math.max(min, view.getContentHeight()))}px`; };
  view.onDidContentSizeChange(fit);
  fit();
  model.onDidChangeContent(() => options.onChange(model.getValue()));
  bindMarkdownKeys(view);
  if (options.onSubmit) view.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => options.onSubmit!());
  if (options.onCancel) view.addCommand(KeyCode.Escape, () => options.onCancel!(), "!suggestWidgetVisible && !findWidgetVisible");
  return {
    getValue: () => model.getValue(),
    setValue: (value) => { if (model.getValue() !== value) model.setValue(value); },
    apply: (command) => applyMarkdownCommand(view, command),
    focus: () => view.focus(),
    dispose: () => { view.dispose(); model.dispose(); },
  };
}
