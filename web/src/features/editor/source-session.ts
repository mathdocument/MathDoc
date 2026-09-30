import { editor as Monaco, Uri, KeyMod, KeyCode } from "monaco-editor";
import {
  loadSourceLanguage,
  setMonacoTheme,
  sourceOptions,
  renderSource,
} from "../../lib/monaco";
import { nativeMonacoScroll } from "../../lib/monaco-scroll";
import type { NodeDetail, SrcBlock } from "../../lib/types";
import { api } from "../../lib/api";
import { errMsg } from "../../lib/format";
import type { Theme } from "../../lib/theme";
import { LatexSession } from "../../lib/latex-session";
import type { PreparedLatexPreview } from "../../lib/latex";
import { removeDraft, setDraftDirty, trackMutation } from "../../lib/unsaved";
import { latexAutocomplete } from "../../lib/latex-completion-widget";
import { ObservableModel } from "../../lib/observable";
export interface SourceEditorSessionProps {
  fnode: string;
  revision: string;
  block: SrcBlock;
  theme: Theme;
  active?: boolean;
  selection?: number;
  latexPreview?: boolean;
  onPreviewChange: (value: boolean) => void;
  preparedLatex?: PreparedLatexPreview;
  onDeleted?: (node: NodeDetail, srctype: string) => void;
  saving?: boolean;
  onChange?: (content: string | null) => void;
  onSave?: () => void;
  onReady?: () => void;
  focusLabel?: string;
  onLatexNavigate?: (fnode: string, label: string) => void;
}
export class SourceEditorSession extends ObservableModel {
  props: SourceEditorSessionProps;
  get fnode() {
    return this.props.fnode;
  }
  get revision() {
    return this.props.revision;
  }
  get block() {
    return this.props.block;
  }
  get theme() {
    return this.props.theme;
  }
  get active() {
    return this.props.active ?? true;
  }
  get latexPreview() {
    return this.props.latexPreview ?? false;
  }
  get preparedLatex() {
    return this.props.preparedLatex;
  }
  get saving() {
    return this.props.saving ?? false;
  }
  get onDeleted() {
    return this.props.onDeleted;
  }
  get onChange() {
    return this.props.onChange;
  }
  get onSave() {
    return this.props.onSave;
  }
  get onReady() {
    return this.props.onReady;
  }
  get focusLabel() {
    return this.props.focusLabel;
  }
  host: HTMLDivElement | null = null;
  scroller!: HTMLDivElement;
  editorView: Monaco.IStandaloneCodeEditor | null = null;
  model: Monaco.ITextModel | null = null;
  disposeScroll: (() => void) | undefined;
  completion:
    | {
        dispose(): void;
      }
    | undefined;
  widgets: HTMLDivElement | undefined;
  ready = false;
  dirty = false;
  deleting = false;
  lastSavedDoc = "";
  error: string | null = null;
  expanded = true;
  get previewing() {
    return this.block.srctype === "latex" && this.latexPreview;
  }
  showPreview = false;
  latex: LatexSession | null = null;
  alive = false;
  initializing = false;
  draftId = Symbol("block draft");
  focusedLabel: string | undefined;
  private revealRequest = 0;
  setDirty = (value: boolean) => {
    this.dirty = value;
    setDraftDirty(this.draftId, value);
    const content =
      value && this.editorView ? this.editorView.getValue() : null;
    this.onChange?.(content);
  };
  ensureEditor = async () => {
    if (!this.alive || this.editorView || this.initializing) return;
    this.initializing = true;
    try {
      const language = await loadSourceLanguage(
        this.block.srctype as "text" | "latex" | "rocq",
      );
      if (!this.alive || this.previewing) return;
      await setMonacoTheme(this.theme);
      if (!this.alive || this.previewing) return;
      this.model = Monaco.createModel(
        this.block.content,
        language,
        Uri.parse(`inmemory://mdc/${this.fnode}/${this.block.srctype}`),
      );
      // Native overflow widgets must escape the block's clipping/stacking context.
      this.widgets = document.createElement("div");
      this.widgets.className = "monaco-editor mdc-editor-widgets";
      document.body.append(this.widgets);
      this.editorView = Monaco.create(this.host!, {
        ...sourceOptions,
        model: this.model,
        ariaLabel: `${this.block.srctype} source`,
        overflowWidgetsDomNode: this.widgets,
        suggestFontSize: 12,
        suggestLineHeight: 28,
        suggest: { showIcons: false, showStatusBar: false },
      });
      this.disposeScroll = nativeMonacoScroll(this.editorView, this.scroller);
      const fit = () =>
        this.scroller.style.setProperty(
          "--source-height",
          `${this.editorView!.getContentHeight()}px`,
        );
      this.editorView.onDidContentSizeChange(fit);
      fit();
      this.editorView.onDidChangeModelContent(() => {
        this.setDirty(this.model!.getValue() !== this.lastSavedDoc);
        this.latex?.schedule(this.model!.getValue());
      });
      this.editorView.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () =>
        this.onSave?.(),
      );
      this.editorView.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () =>
        this.onSave?.(),
      );
      if (this.latex)
        this.completion = latexAutocomplete(this.latex, this.editorView);
      this.reveal();
    } catch (e) {
      if (this.alive) {
        this.error = errMsg(e);
        this.onReady?.();
      }
    } finally {
      this.initializing = false;
    }
  };
  private reveal() {
    const request = ++this.revealRequest;
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (!this.alive || request !== this.revealRequest || !this.editorView) return;
      renderSource(this.editorView);
      this.ready = true;
      this.syncView();
      this.onReady?.();
    }));
  }
  onDelete = async () => {
    if (this.saving || this.deleting) return;
    if (!confirm(`Delete the ${this.block.srctype} block from this node?`))
      return;
    const targetFnode = this.fnode;
    const targetSrctype = this.block.srctype;
    const targetRevision = this.revision;
    this.error = null;
    this.deleting = true;
    const clearMutation = trackMutation();
    const isCurrent = () => this.alive && this.fnode === targetFnode;
    try {
      const node = await api.deleteBlock(
        targetFnode,
        targetSrctype,
        targetRevision,
      );
      if (!isCurrent()) return;
      this.setDirty(false);
      clearMutation();
      this.onDeleted?.(node, targetSrctype);
    } catch (e) {
      if (isCurrent()) this.error = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) this.deleting = false;
    }
  };
  toggleExpand = () => {
    this.expanded = !this.expanded;
    this.syncActive();
    this.syncView();
    this.syncLayout();
  };
  syncView() {
    if (this.previewing) this.showPreview = true;
    else if (this.ready) this.showPreview = false;
    if (!this.active || !this.expanded) {
      this.onReady?.();
      return;
    }
    if (!this.previewing) void this.ensureEditor();
    else if (this.latex?.preview || this.latex?.error) this.onReady?.();
  }
  syncTheme() {
    const next = this.theme;
    if (this.ready) void setMonacoTheme(next);
  }
  syncActive() {
    this.latex?.setActive(this.active && this.expanded);
  }
  syncLayout() {
    if (!this.showPreview && this.ready)
      requestAnimationFrame(() => {
        if (this.alive && this.editorView) renderSource(this.editorView);
      });
  }
  syncFocus() {
    if (
      this.focusLabel &&
      this.latex &&
      this.focusedLabel !== this.focusLabel
    ) {
      this.focusedLabel = this.focusLabel;
      this.expanded = true;
      this.props.onPreviewChange(true);
    }
  }
  destroy() {
    this.alive = false;
    this.stopLatex?.();
    this.latex?.destroy();
    removeDraft(this.draftId);
    this.disposeEditor();
  }
  private disposeEditor() {
    this.revealRequest++;
    this.completion?.dispose();
    this.completion = undefined;
    this.disposeScroll?.();
    this.disposeScroll = undefined;
    this.editorView?.dispose();
    this.widgets?.remove();
    this.widgets = undefined;
    this.model?.dispose();
    this.model = null;
    this.editorView = null;
  }
  syncSource(discardDraft = false) {
    const nextContent = this.block.content;
    if (this.lastSavedDoc === nextContent && !discardDraft) return;
    this.error = null;
    this.lastSavedDoc = nextContent;
    if (
      this.editorView &&
      (discardDraft || !this.dirty) &&
      this.editorView.getValue() !== nextContent
    )
      this.editorView.setValue(nextContent);
    if (!this.editorView && this.latex && this.latex.source !== nextContent)
      this.latex.schedule(nextContent);
    this.setDirty(
      this.editorView !== null &&
        this.editorView.getValue() !== this.lastSavedDoc,
    );
  }
  private stopLatex?: () => void;
  private resetLatex() {
    this.stopLatex?.();
    this.latex?.destroy();
    this.completion?.dispose();
    this.completion = undefined;
    this.latex = this.block.srctype === "latex"
      ? new LatexSession(this.fnode, this.block.content, this.preparedLatex)
      : null;
    this.stopLatex = this.latex?.subscribe(() => {
      this.syncView();
      this.notifyListeners();
    });
    if (this.latex && this.editorView)
      this.completion = latexAutocomplete(this.latex, this.editorView);
  }
  private switchNode() {
    this.ready = false;
    this.error = null;
    this.deleting = false;
    this.expanded = true;
    this.focusedLabel = undefined;
    this.lastSavedDoc = this.block.content;
    this.showPreview = this.previewing;
    this.setDirty(false);
    if (this.previewing) this.disposeEditor();
    else if (this.editorView && this.model) {
      const previous = this.model;
      this.model = Monaco.createModel(this.block.content, previous.getLanguageId(),
        Uri.parse(`inmemory://mdc/${this.fnode}/${this.block.srctype}`));
      this.editorView.setModel(this.model);
      previous.dispose();
      this.reveal();
    }
    this.resetLatex();
  }
  attach(host: HTMLDivElement, scroller: HTMLDivElement) {
    if (this.alive && this.host === host && this.scroller === scroller) return;
    this.host = host;
    this.scroller = scroller;
    this.alive = true;
    this.lastSavedDoc = this.block.content;
    this.resetLatex();
    this.syncActive();
    this.syncView();
    this.syncFocus();
  }
  update(props: SourceEditorSessionProps) {
    const previous = this.props;
    this.props = props;
    if (previous.fnode !== props.fnode) this.switchNode();
    this.syncSource(previous.selection !== props.selection);
    if (previous.fnode === props.fnode && previous.selection !== props.selection && this.editorView) {
      this.ready = false;
      this.reveal();
    }
    this.syncActive();
    this.syncView();
    this.syncFocus();
    if (previous.theme !== props.theme) this.syncTheme();
    if (previous.latexPreview !== props.latexPreview) this.syncLayout();
  }
  constructor(props: SourceEditorSessionProps) {
    super();
    this.props = props;
    this.observe(
      "ready",
      "dirty",
      "deleting",
      "error",
      "expanded",
      "showPreview",
      "latex",
    );
  }
}
