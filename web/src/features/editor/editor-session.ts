import { type NodeDetail } from "../../lib/types";
import type { Theme } from "../../lib/theme";
import { errMsg } from "../../lib/format";
import { nodeNameError } from "../../lib/node-name";
import { api } from "../../lib/api";
import {
  hasPendingMutations,
  removeDraft,
  setDraftDirty,
  trackMutation,
} from "../../lib/unsaved";
import type { LoadState } from "../../lib/node-session";
import { ObservableModel } from "../../lib/observable";
export interface EditorSessionProps {
  load: LoadState;
  theme: Theme;
  active?: boolean;
  selection?: number;
  latexPreview?: boolean;
  onPreviewChange: (value: boolean) => void;
  onRefresh?: (node: NodeDetail, graphChanged?: boolean) => void;
  onReady?: () => void;
  latexTarget?: {
    fnode: string;
    label: string;
  } | null;
  onLatexNavigate?: (fnode: string, label: string) => void;
}
export class EditorSession extends ObservableModel {
  props: EditorSessionProps;
  get load() {
    return this.props.load;
  }
  get theme() {
    return this.props.theme;
  }
  get active() {
    return this.props.active ?? true;
  }
  get selection() {
    return this.props.selection ?? 0;
  }
  get latexPreview() {
    return this.props.latexPreview ?? false;
  }
  get onRefresh() {
    return this.props.onRefresh;
  }
  get onReady() {
    return this.props.onReady;
  }
  get latexTarget() {
    return this.props.latexTarget;
  }
  get onLatexNavigate() {
    return this.props.onLatexNavigate;
  }
  get node() {
    return this.load.kind === "ready" ? this.load.node : null;
  }
  editingTitle = false;
  titleDraft = "";
  titleError: string | null = null;
  titleSaving = false;
  titleInputEl: HTMLInputElement | null = null;
  titleDraftId = Symbol("title draft");
  displayedFnode: string | null = null;
  titleRequest = 0;
  readyReported = false;
  readyBlocks = new Set<string>();
  BlockEditorComponent: typeof import("./BlockEditor").default | null = null;
  blockEditorPromise: Promise<typeof import("./BlockEditor").default> | null =
    null;
  editorLoadError: string | null = null;
  alive = true;
  drafts: Record<string, string> = {};
  saving = false;
  saveError: string | null = null;
  saveRequest = 0;
  get changes() {
    return Object.fromEntries(
      (this.node?.blocks ?? [])
        .filter(
          (block) =>
            this.drafts[block.srctype] !== undefined &&
            this.drafts[block.srctype] !== block.content,
        )
        .map((block) => [block.srctype, this.drafts[block.srctype]!]),
    );
  }
  updateDraft = (fnode: string, srctype: string, content: string | null) => {
    if (this.node?.fnode !== fnode) return;
    if (this.drafts[srctype] === (content ?? undefined)) return;
    const next = { ...this.drafts };
    if (content === null) delete next[srctype];
    else next[srctype] = content;
    this.drafts = next;
  };
  saveNode = async () => {
    if (
      !this.node ||
      this.saving ||
      hasPendingMutations() ||
      Object.keys(this.changes).length === 0
    )
      return;
    const target = this.node,
      submitted = { ...this.changes },
      request = ++this.saveRequest;
    const isCurrent = () =>
      this.alive &&
      request === this.saveRequest &&
      this.node?.fnode === target.fnode;
    this.saving = true;
    this.saveError = null;
    const release = trackMutation();
    try {
      const updated = await api.putBlocks(
        target.fnode,
        submitted,
        target.revision,
      );
      if (isCurrent()) this.applyBlockUpdate(updated, true);
    } catch (error) {
      if (isCurrent()) this.saveError = errMsg(error);
    } finally {
      release();
      if (isCurrent()) this.saving = false;
    }
  };
  saveShortcut = (event: KeyboardEvent) => {
    if (
      !this.active ||
      event.defaultPrevented ||
      event.isComposing ||
      document.querySelector('[role="dialog"]') ||
      !(event.ctrlKey || event.metaKey) ||
      event.key.toLowerCase() !== "s"
    )
      return;
    event.preventDefault();
    void this.saveNode();
  };
  applyBlockUpdate = (updated: NodeDetail, graphChanged = false) => {
    if (this.load.kind !== "ready" || this.load.node.fnode !== updated.fnode)
      return;
    this.onRefresh?.(updated, graphChanged);
  };
  reportReady = () => {
    if (this.readyReported) return;
    this.readyReported = true;
    this.onReady?.();
  };
  reportBlockReady = (srctype: string) => {
    if (this.load.kind !== "ready" || this.readyReported) return;
    this.readyBlocks.add(srctype);
    if (this.readyBlocks.size === this.load.node.blocks.length)
      this.reportReady();
  };
  ensureBlockEditorLoaded = () => {
    if (this.BlockEditorComponent || this.blockEditorPromise) return;
    this.editorLoadError = null;
    this.blockEditorPromise = import("./BlockEditor").then(
      (module) => module.default,
    );
    void this.blockEditorPromise
      .then((component) => {
        if (this.alive) this.BlockEditorComponent = component;
      })
      .catch((error) => {
        if (!this.alive) return;
        this.editorLoadError = errMsg(error);
        this.reportReady();
      })
      .finally(() => {
        this.blockEditorPromise = null;
      });
  };
  startEditTitle = () => {
    if (this.load.kind !== "ready") return;
    this.titleDraft = this.load.node.name;
    this.editingTitle = true;
  };
  saveTitle = async () => {
    if (this.load.kind !== "ready" || this.titleSaving) return;
    const newTitle = this.titleDraft;
    const invalid = nodeNameError(newTitle);
    if (invalid) {
      this.titleError = invalid;
      return;
    }
    if (newTitle === this.load.node.name) {
      this.editingTitle = false;
      this.titleError = null;
      return;
    }
    const targetFnode = this.load.node.fnode;
    const request = ++this.titleRequest;
    const isCurrent = () =>
      request === this.titleRequest &&
      this.load.kind === "ready" &&
      this.load.node.fnode === targetFnode;
    this.titleSaving = true;
    const clearMutation = trackMutation();
    this.titleError = null;
    try {
      const updated = await api.putName(
        targetFnode,
        newTitle,
        this.load.node.revision,
      );
      if (!isCurrent() || this.load.kind !== "ready") return;
      this.onRefresh?.(updated, true);
      this.editingTitle = false;
    } catch (e) {
      if (isCurrent()) this.titleError = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) this.titleSaving = false;
    }
  };
  cancelEditTitle = () => {
    this.editingTitle = false;
    this.titleError = null;
  };
  reportEmpty() {
    if (
      this.load.kind === "error" ||
      (this.load.kind === "ready" && this.load.node.blocks.length === 0)
    ) {
      this.reportReady();
    }
  }
  loadEditors() {
    if (
      this.load.kind === "ready" &&
      this.load.node.blocks.some((block) => block.srctype !== "lean")
    )
      this.ensureBlockEditorLoaded();
  }
  resetNode() {
    const fnode = this.load.kind === "ready" ? this.load.node.fnode : null;
    if (fnode === this.displayedFnode) return;
    this.displayedFnode = fnode;
    this.readyReported = false;
    this.readyBlocks.clear();
    this.titleRequest++;
    this.editingTitle = false;
    this.titleSaving = false;
    this.titleError = null;
    this.saveRequest++;
    this.drafts = {};
    this.saving = false;
    this.saveError = null;
  }
  trackTitle() {
    const isDirty =
      this.editingTitle &&
      this.load.kind === "ready" &&
      this.titleDraft !== this.load.node.name;
    const value = isDirty ? this.titleDraft : null;
    if (value === this.lastTitleDraft) return;
    this.lastTitleDraft = value;
    setDraftDirty(this.titleDraftId, isDirty);
  }
  destroy() {
    this.alive = false;
    this.titleRequest++;
    removeDraft(this.titleDraftId);
  }
  focusTitle() {
    if (this.editingTitle) this.titleInputEl?.focus();
  }
  private lastTitleDraft: string | null | undefined;
  update(props: EditorSessionProps) {
    this.props = props;
    this.resetNode();
    this.reportEmpty();
    this.loadEditors();
    this.trackTitle();
    this.focusTitle();
  }
  attachTitle = (input: HTMLInputElement | null) => {
    this.titleInputEl = input;
  };
  constructor(props: EditorSessionProps) {
    super();
    this.props = props;
    this.observe(
      "editingTitle",
      "titleDraft",
      "titleError",
      "titleSaving",
      "BlockEditorComponent",
      "editorLoadError",
      "drafts",
      "saving",
      "saveError",
    );
  }
}
