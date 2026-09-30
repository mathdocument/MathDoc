import type { NodeDetail, SrcBlock } from "../../lib/types";
import type { Theme } from "../../lib/theme";
import { api, type LeanCheckResult } from "../../lib/api";
import { errMsg } from "../../lib/format";
import { removeDraft, setDraftDirty, trackMutation } from "../../lib/unsaved";
import { ObservableModel } from "../../lib/observable";
export interface LeanEditorSessionProps {
  fnode: string;
  revision: string;
  module?: string;
  block?: SrcBlock;
  theme: Theme;
  active?: boolean;
  selection?: number;
  onDeleted?: (node: NodeDetail, srctype: string) => void;
  saving?: boolean;
  onChange?: (content: string | null) => void;
  onSave?: () => void;
  onCertified?: (node: NodeDetail) => void;
  onReady?: () => void;
}
export class LeanEditorSession extends ObservableModel {
  props: LeanEditorSessionProps;
  get fnode() {
    return this.props.fnode;
  }
  get revision() {
    return this.props.revision;
  }
  get module() {
    return this.props.module;
  }
  get block() {
    return this.props.block;
  }
  get theme() {
    return this.props.theme;
  }
  get selection() {
    return this.props.selection ?? 0;
  }
  get saving() {
    return this.props.saving ?? false;
  }
  get onChange() {
    return this.props.onChange;
  }
  get onSave() {
    return this.props.onSave;
  }
  get onDeleted() {
    return this.props.onDeleted;
  }
  get onCertified() {
    return this.props.onCertified;
  }
  get onReady() {
    return this.props.onReady;
  }
  frame: HTMLIFrameElement | undefined;
  session: string | null = null;
  mounted = false;
  content = "";
  baseline = "";
  get dirty() {
    return this.content !== this.baseline;
  }
  deleting = false;
  validating = false;
  get busy() {
    return this.saving || this.deleting;
  }
  error: string | null = null;
  result: LeanCheckResult | null = null;
  ready = false;
  runtimeReady = false;
  opening = false;
  stopping = false;
  reconnects = 0;
  reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  connection = 0;
  closing = new Map<string, () => void>();
  progress = "";
  generation = 0;
  openedNode = "";
  openedModule: string | undefined;
  openedSelection = -1;
  initialTheme: Theme = "light";
  alive = true;
  expanded = true;
  draft = Symbol("Lean draft");
  selectNode = () => {
    if (this.runtimeReady && this.block)
      this.frame?.contentWindow?.postMessage(
        {
          type: "lean-select",
          fnode: this.fnode,
          revision: this.revision,
          generation: this.generation,
          module: this.module,
          source: this.content,
        },
        location.origin,
      );
  };
  open = async () => {
    if (!this.block || !this.runtimeReady) return;
    const initial = {
      fnode: this.fnode,
      revision: this.revision,
      generation: this.generation,
      module: this.module,
      source: this.content,
    };
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    const current = ++this.connection;
    const previous = this.closeSession();
    this.opening = true;
    this.error = null;
    this.result = null;
    this.frame?.contentWindow?.postMessage(
      { type: "lean-starting" },
      location.origin,
    );
    try {
      await previous;
      // Allocation prepares one module. If navigation overtakes it, retire that
      // session before connecting a model that the backend has not prepared.
      while (this.alive && this.connection === current) {
        const selected = this.block
          ? {
              fnode: this.fnode,
              revision: this.revision,
              generation: this.generation,
              module: this.module,
              source: this.content,
            }
          : initial;
        const response = await api.leanSession(
          selected.fnode,
          selected.revision,
        );
        if (!this.alive || this.connection !== current) {
          void api.closeLeanSession(response.id).catch(console.warn);
          return;
        }
        if (this.block && this.fnode !== selected.fnode) {
          await api.closeLeanSession(response.id);
          continue;
        }
        this.session = response.id;
        // A node without Lean hides the editor; it must not cancel its start.
        this.frame?.contentWindow?.postMessage(
          {
            type: "lean-start",
            id: this.session,
            ...(this.block
              ? {
                  fnode: this.fnode,
                  revision: this.revision,
                  generation: this.generation,
                  module: this.module,
                  source: this.content,
                }
              : selected),
          },
          location.origin,
        );
        break;
      }
    } catch (e) {
      if (this.alive && this.connection === current) {
        this.error = errMsg(e);
        this.frame?.contentWindow?.postMessage(
          { type: "lean-stop" },
          location.origin,
        );
      }
    } finally {
      if (this.connection === current) this.opening = false;
    }
  };
  refresh = () => {
    if (!this.session) {
      this.reconnects = 0;
      void this.open();
      return;
    }
    this.error = null;
    this.result = null;
    this.frame?.contentWindow?.postMessage(
      {
        type: "lean-recheck",
        fnode: this.fnode,
        revision: this.revision,
        generation: this.generation,
        module: this.module,
        source: this.content,
      },
      location.origin,
    );
  };
  stop = async () => {
    const current = ++this.connection;
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    this.stopping = true;
    this.opening = false;
    this.validating = false;
    this.error = null;
    this.result = null;
    this.progress = "";
    try {
      await this.closeSession();
    } catch (e) {
      if (this.alive && this.connection === current) this.error = errMsg(e);
    } finally {
      if (this.alive && this.connection === current) this.stopping = false;
    }
  };
  disconnected = (reason: string) => {
    this.error = reason;
    this.progress = "";
    // One automatic attempt per editor lifetime; repeated crashes need an explicit
    // retry. The editor and its unsaved model survive the connection change.
    if (!this.reconnectTimer && this.reconnects < 1 && this.block) {
      this.reconnects++;
      this.progress = "Reconnecting Lean...";
      this.reconnectTimer = setTimeout(() => {
        if (this.alive) void this.open();
      }, 500);
    }
  };
  closeSession = () => {
    const id = this.session;
    this.session = null;
    const stopped =
      id && this.alive && this.runtimeReady
        ? new Promise<void>((resolve) => {
            const done = () => {
              clearTimeout(timer);
              this.closing.delete(id);
              resolve();
            };
            // Do not let a wedged browser block the next explicit start.
            const timer = setTimeout(done, 1500);
            this.closing.set(id, done);
          })
        : Promise.resolve();
    this.frame?.contentWindow?.postMessage(
      { type: "lean-stop", session: id },
      location.origin,
    );
    // Kill the native process immediately, even during initialization/checking.
    return Promise.all([stopped, id ? api.closeLeanSession(id) : undefined]);
  };
  certified = async (checked: LeanCheckResult) => {
    const current = this.generation,
      connection = this.connection,
      node = this.fnode;
    const isCurrent = () =>
      this.alive && this.generation === current && this.connection === connection &&
      this.revision === checked.revision && !this.dirty;
    if (!isCurrent() || checked.fnode !== node) return;
    this.result = checked;
    this.error = null;
    try {
      const view = await api.nodeView(node);
      if (!isCurrent()) return;
      if (view.node.revision !== checked.revision) {
        this.result = null;
        throw new Error("Node changed externally; refresh before continuing");
      }
      this.onCertified?.(view.node);
    } catch (e) {
      if (isCurrent()) this.error = errMsg(e);
    }
  };
  remove = async () => {
    if (this.busy || !confirm("Delete the Lean block from this node?")) return;
    this.deleting = true;
    const release = trackMutation();
    try {
      const updated = await api.deleteBlock(this.fnode, "lean", this.revision);
      this.onDeleted?.(updated, "lean");
    } catch (e) {
      this.error = errMsg(e);
    } finally {
      this.deleting = false;
      release();
    }
  };
  message = (event: MessageEvent) => {
    if (
      event.origin !== location.origin ||
      event.source !== this.frame?.contentWindow
    )
      return;
    if (event.data?.type === "lean-stopped") {
      this.closing.get(event.data.session)?.();
      return;
    }
    if (event.data?.session && event.data.session !== this.session) return;
    if (event.data?.type === "lean-runtime-ready") {
      this.runtimeReady = true;
      this.selectNode();
      this.syncSaved();
      return;
    }
    if (event.data?.type === "lean-disconnected") {
      if (this.session && !this.stopping)
        this.disconnected(String(event.data.value));
      return;
    }
    if (event.data?.type === "lean-error") {
      this.error = String(event.data.value);
      this.onReady?.();
      return;
    }
    if (event.data?.fnode !== this.fnode || !this.block) return;
    switch (event.data?.type) {
      case "lean-change":
        if (typeof event.data.value === "string") {
          this.content = event.data.value;
          this.result = null;
          this.syncDraft();
        }
        break;
      case "lean-save":
        this.onSave?.();
        break;
      case "lean-certified":
        void this.certified(event.data.value);
        break;
      case "lean-validating":
        this.validating = !!event.data.value;
        break;
      case "lean-validation-error":
        this.error = String(event.data.value);
        break;
      case "lean-ready":
        if (event.data.generation === this.generation) {
          this.ready = true;
          this.onReady?.();
        }
        break;
      case "lean-progress":
        this.progress = String(event.data.value);
        break;
    }
  };
  private lastDraft: string | null | undefined;
  syncDraft() {
    const value = this.block && this.dirty ? this.content : null;
    if (value === this.lastDraft) return;
    this.lastDraft = value;
    setDraftDirty(this.draft, value !== null);
    this.onChange?.(value);
  }
  syncSelection() {
    const node = this.block ? this.fnode : "";
    if (node === this.openedNode && this.selection === this.openedSelection) {
      if (this.module !== this.openedModule) {
        this.openedModule = this.module;
        // A rename changes the native URI, but must retain the current draft.
        this.generation++;
        this.ready = false;
        this.result = null;
        this.error = null;
        this.selectNode();
      }
      return;
    }
    this.openedModule = this.module;
    // Navigation has already confirmed discarding edits. Reset the old native
    // document as well, so its retained worker never carries an abandoned draft.
    this.frame?.contentWindow?.postMessage(
      { type: "lean-source", fnode: this.openedNode, value: this.baseline },
      location.origin,
    );
    this.openedNode = node;
    this.openedSelection = this.selection;
    this.generation++;
    this.ready = false;
    this.error = null;
    this.result = null;
    this.progress = "";
    this.validating = false;
    this.content = this.block?.content ?? "";
    this.baseline = this.content;
    if (node) {
      if (!this.mounted && !this.opening) {
        this.initialTheme = this.theme;
        this.mounted = true;
      }
      this.selectNode();
    } else
      this.frame?.contentWindow?.postMessage(
        { type: "lean-cancel" },
        location.origin,
      );
  }
  syncTheme() {
    this.frame?.contentWindow?.postMessage(
      { type: "lean-theme", value: this.theme },
      location.origin,
    );
  }
  syncSource() {
    const next = this.block?.content;
    if (next === undefined) return;
    if (next === this.baseline) return;
    if (!this.dirty) {
      this.content = next;
      this.frame?.contentWindow?.postMessage(
        { type: "lean-source", fnode: this.fnode, value: next },
        location.origin,
      );
    }
    this.baseline = next;
    this.result = null;
  }
  syncSaved() {
    if (this.runtimeReady && this.block)
      this.frame?.contentWindow?.postMessage(
        {
          type: "lean-saved",
          fnode: this.fnode,
          revision: this.revision,
          source: this.block.content,
        },
        location.origin,
      );
  }
  destroy() {
    clearTimeout(this.reconnectTimer);
    this.alive = false;
    this.generation++;
    this.connection++;
    void this.closeSession().catch(console.warn);
    removeDraft(this.draft);
  }
  update(props: LeanEditorSessionProps) {
    const previous = this.props;
    this.props = props;
    this.syncSelection();
    this.syncSource();
    this.syncDraft();
    if (previous.theme !== props.theme) this.syncTheme();
    if (
      previous.revision !== props.revision ||
      previous.block?.content !== props.block?.content
    )
      this.syncSaved();
  }
  attachFrame = (frame: HTMLIFrameElement | null) => {
    this.frame = frame ?? undefined;
    if (frame) this.syncTheme();
  };
  constructor(props: LeanEditorSessionProps) {
    super();
    this.props = props;
    this.observe(
      "session",
      "mounted",
      "content",
      "baseline",
      "deleting",
      "validating",
      "error",
      "result",
      "ready",
      "runtimeReady",
      "opening",
      "stopping",
      "progress",
      "initialTheme",
      "expanded",
    );
  }
}
