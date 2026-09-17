import { LeanMonaco, type LeanClient, type LeanMonacoOptions } from "lean4monaco";
import { CancellationTokenSource, Uri, KeyCode, KeyMod, editor as MonacoEditor } from "monaco-editor";
import { createModelReference } from "vscode/monaco";
import { getService, ICommandService } from "vscode/services";
import { FileUri } from "lean4monaco/dist/vscode-lean4/vscode-lean4/src/utils/exturi";
import { CloseAction, ErrorAction } from "vscode-languageclient/lib/common/client.js";
import { projectPath } from "./lib/project-path";
import { chainEditorScroll } from "./lib/editor-scroll";
import { nativeMonacoScroll } from "./lib/monaco-scroll";
import { initializeMonaco, sourceOptions, setMonacoTheme, renderSource } from "./lib/monaco";
import { leanSourcePath } from "./lib/lean-module";

const params = new URLSearchParams(location.search);
let id: string | null = null;
function applyTheme(theme: string) {
  document.body.style.backgroundColor = theme === "light" ? "#fff" : "#1f1f1f";
  void setMonacoTheme(theme === "light" ? "light" : "dark");
}
class BrowserLean extends LeanMonaco {
  socket!: ReturnType<LeanMonaco["getWebSocketOptions"]>;
  protected getWebSocketOptions(options: LeanMonacoOptions) {
    // The native client factory reads these options on each explicit start.
    return this.socket = super.getWebSocketOptions(options);
  }
  protected getExtensionManifest() {
    // LeanMonaco installs browser providers itself; its desktop entry cannot run here.
    return { ...super.getExtensionManifest(), main: undefined, browser: undefined, activationEvents: [] };
  }
}
interface Document { fnode: string; filename: string; source: string; revision: string; environment_key: string }
interface OpenDocument {
  document: Document;
  reference: Awaited<ReturnType<typeof createModelReference>>;
  view: MonacoEditor.ICodeEditorViewState | null;
  progress: string;
  validating?: boolean;
  validated?: string;
  checkGeneration?: number;
}
const runtime = new BrowserLean();
let editor: MonacoEditor.IStandaloneCodeEditor;
let disposeScroll: (() => void) | undefined;
const documents = new Map<string, OpenDocument>();
const preparingProgress = new Map<string, string>();
let selected: OpenDocument | undefined;
let shownFnode = "";
let preview: MonacoEditor.ITextModel | undefined;
let updating = false;
let selection = new AbortController();
let connectionFailure: string | undefined;
let resolveClient: (client: LeanClient) => void;
let clientReady = new Promise<LeanClient>(resolve => { resolveClient = resolve; });
let connecting: Promise<[boolean, LeanClient | undefined]> | undefined;
let stopped = Promise.resolve();
let connection = 0;
const send = (type: string, value?: unknown, generation?: number) => parent.postMessage({ type, value, fnode: shownFnode, generation, session: id }, location.origin);
function progressText(processing: { range: { start: { line: number } } }[]) {
  return processing.length === 0 ? "Lean editor ready" :
    processing.some(item => item.range.start.line === 0) ? "Loading Lean imports…" : "Lean is checking edits…";
}
function showProgress() { send("lean-progress", !id || connectionFailure ? "" : preview ? "Preparing Lean environment…" : selected?.progress ?? ""); }
function reveal(generation: number) {
  const current = selection;
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (current.signal.aborted) return;
    renderSource(editor);
    send("lean-ready", undefined, generation);
  }));
}
function disconnected(reason: string) {
  if (!id || connectionFailure) return;
  connectionFailure = reason;
  selection.abort(new Error(reason));
  send("lean-disconnected", reason);
}
async function validate(entry: OpenDocument) {
  if (!id) return;
  const session = id;
  const key = () => `${id}:${entry.checkGeneration ?? 0}:${entry.document.revision}:${entry.document.environment_key}:${entry.reference.object.textEditorModel?.getVersionId()}`;
  if (documents.get(entry.document.filename) !== entry || entry.validating || entry.validated === key() || entry.progress !== "Lean editor ready" ||
      entry.reference.object.textEditorModel?.getValue() !== entry.document.source) return;
  const requested = key(), { fnode, revision } = entry.document;
  const post = (type: string, value?: unknown) => { if (id === session) parent.postMessage({ type, value, fnode, session }, location.origin); };
  entry.validating = true; post("lean-validating", true);
  try {
    const client = await clientReady;
    // The bridge obtains the exact LSP version from native document notifications.
    const state: { uri: string; version: number; module: unknown } = await client.sendRequest("mdc/validationState", { fnode, revision });
    await client.sendRequest("textDocument/waitForDiagnostics", { uri: state.uri, version: state.version });
    if (key() !== requested) return;
    await client.sendRequest("$/lean/moduleHierarchy/imports", { module: state.module });
    if (key() !== requested) return;
    const result = await client.sendRequest("mdc/certify", { fnode, revision, version: state.version });
    if (key() === requested) { entry.validated = requested; post("lean-certified", result); }
  } catch (e) {
    // Changes can overtake a validation request. The next native progress event
    // retries the latest saved version; stale evidence is never published.
    if (key() === requested && !/draft differs|not open yet|not complete|node changed/.test(String(e))) {
      post("lean-validation-error", String(e));
    }
  } finally {
    entry.validating = false; post("lean-validating", false);
    if (key() !== requested) void validate(entry);
  }
}
function showNode(fnode: string, source: string, generation: number, module: string) {
  if (selected && !preview) selected.view = editor.saveViewState();
  const oldPreview = preview;
  selected = [...documents.values()].find(entry => entry.document.fnode === fnode);
  shownFnode = fnode;
  // While connected, cold nodes use temporary models until their files are
  // prepared. Offline models already have the URI they will use on attachment.
  const uri = id ? Uri.parse(`inmemory://mdc/${fnode}/${generation}.lean`) : Uri.file(leanSourcePath(module));
  preview = selected ? undefined : oldPreview?.uri.toString() === uri.toString() ? oldPreview : MonacoEditor.createModel(source, "lean4", uri);
  updating = true;
  editor.setModel(selected?.reference.object.textEditorModel ?? preview!);
  if (editor.getValue() !== source) editor.setValue(source);
  if (selected?.view) editor.restoreViewState(selected.view);
  updating = false;
  if (oldPreview !== preview && oldPreview !== editor.getModel()) oldPreview?.dispose();
  if (!id) {
    // An offline navigation must not reopen an unprepared previous file when
    // the next server starts. Keep only the visible model and its undo history.
    stopped = Promise.all([stopped, ...[...documents.keys()].filter(key => key !== selected?.document.filename).map(evict)]).then(() => {});
  }
  editor.focus();
  document.getElementById("infoview-pending")!.hidden = !id || !preview;
  showProgress();
  reveal(generation);
}
function error(error: unknown) {
  document.getElementById("error")!.textContent = String(error);
  send("lean-error", String(error));
}
async function evict(filename: string) {
  const old = documents.get(filename)!;
  documents.delete(filename);
  const model = old.reference.object.textEditorModel;
  // Clear Monaco's dirty flag before releasing its reference, so VS Code closes
  // the document and Lean releases its worker. This never writes to the database.
  await old.reference.object.revert({ soft: true });
  old.reference.dispose();
  // A model created before LSP attachment is owned by us, not the reference.
  // Dispose it too so VS Code cannot reopen this retired file on the next start.
  if (model && !model.isDisposed()) model.dispose();
}
// Cancellation must settle locally even if an LSP peer never answers its request.
async function abortable<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  let cancel!: () => void;
  try {
    return await Promise.race([work, new Promise<never>((_, reject) => {
      cancel = () => reject(signal.reason);
      signal.addEventListener("abort", cancel, { once: true });
      if (signal.aborted) cancel();
    })]);
  } finally { signal.removeEventListener("abort", cancel); }
}
async function selectNode(fnode: string, revision: string, generation: number, signal: AbortSignal, recheck = false) {
  const client = await abortable(clientReady, signal);
  signal.throwIfAborted();
  const token = new CancellationTokenSource();
  const cancel = () => token.cancel();
  signal.addEventListener("abort", cancel, { once: true });
  let next: Document;
  try {
    next = await abortable(client.sendRequest("mdc/selectNode", { fnode, revision }, token.token), signal);
  } finally { signal.removeEventListener("abort", cancel); token.dispose(); }
  if (signal.aborted) return;
  document.getElementById("error")!.textContent = "";

  let entry = documents.get(next.filename);
  const restart = entry && (recheck || entry.document.environment_key !== next.environment_key);
  if (!entry) {
    // ponytail: two hot documents; larger sets would retain another full Mathlib
    // environment per file. Evicted files keep Lake's shared compiled artifacts.
    if (documents.size >= 2) {
      const oldest = documents.keys().next().value!;
      await evict(oldest);
      if (signal.aborted) return;
    }
    const reference = await createModelReference(Uri.file(next.filename), next.source);
    if (signal.aborted) {
      await reference.object.revert({ soft: true }); reference.dispose(); return;
    }
    entry = { document: next, reference, view: null, progress: "Loading Lean imports…" };
  }
  documents.delete(next.filename);
  documents.set(next.filename, entry);
  entry.document = next;
  const source = editor.getValue();
  const view = editor.saveViewState();
  const focused = editor.hasTextFocus();
  const model = entry.reference.object.textEditorModel;
  if (!model) throw new Error("Lean document closed while switching nodes");
  selected = entry;
  updating = true;
  editor.setModel(model);
  if (model.getValue() !== source) {
    model.pushStackElement();
    model.pushEditOperations([], [{ range: model.getFullModelRange(), text: source }], () => null);
    model.pushStackElement();
  }
  if (view) editor.restoreViewState(view);
  updating = false;
  if (preview !== model) preview?.dispose();
  preview = undefined;
  if (!runtime.infoProvider!.isOpen()) void getService(ICommandService).then(commands => { if (!signal.aborted) return commands.executeCommand("lean4.displayGoal"); });
  document.getElementById("infoview-pending")!.hidden = true;
  if (focused) editor.focus();
  if (restart) {
    entry.checkGeneration = (entry.checkGeneration ?? 0) + 1;
    entry.progress = "Rechecking Lean…";
    // Reopen this file in the existing language client, including when collapsed
    // or unfocused. Other documents and the session's server remain alive.
    runtime.clientProvider!.restartFile(FileUri.fromUriOrError(model.uri));
  } else {
    // A retained model can finish checking before the selection reply arrives.
    const progress = preparingProgress.get(next.filename);
    if (progress) entry.progress = progress;
  }
  showProgress();
  reveal(generation);
  void validate(entry);
}
function stopSession(session: string | null) {
  connection++; id = null; selection.abort();
  document.body.classList.add("static");
  document.getElementById("error")!.textContent = "";
  showProgress();
  // Closing the native view cancels its RPC requests and releases sessions.
  // The source editor/model stays mounted; only Infoview is recreated on start.
  const closedView = runtime.infoProvider?.isOpen() ? getService(ICommandService).then(commands => commands.executeCommand("lean4.toggleInfoview")) : undefined;
  const pending = connecting;
  stopped = stopped.then(async () => {
    await closedView;
    await pending;
    for (const client of runtime.clientProvider!.getClients()) await client.stop();
    for (const key of [...documents.keys()]) if (key !== selected?.document.filename) await evict(key);
    for (const entry of documents.values()) {
      entry.progress = ""; entry.validated = undefined;
      const model = entry.reference.object.textEditorModel;
      if (model) for (const owner of new Set(MonacoEditor.getModelMarkers({resource: model.uri}).map(marker => marker.owner))) MonacoEditor.setModelMarkers(model, owner, []);
    }
  }).catch(error);
  void stopped.then(() => parent.postMessage({type: "lean-stopped", session}, location.origin));
}
interface Selection { fnode: string; revision: string; generation: number; source: string; module: string }
let currentNode: Selection;
function choose(data: Selection, recheck = false) {
  currentNode = data;
  selection.abort(); selection = new AbortController();
  showNode(data.fnode, data.source, data.generation, data.module);
  prepare(data, recheck);
}
function prepare({fnode, revision, generation}: Selection, recheck = false) {
  const current = selection;
  preparingProgress.clear();
  if (!id) return;
  if (connectionFailure) { error(connectionFailure); return; }
  const timer = setTimeout(() => current.abort(new DOMException("Lean node preparation timed out", "TimeoutError")), 30000);
  void abortable(selectNode(fnode, revision, generation, current.signal, recheck), current.signal).catch(e => {
    if (current !== selection) return;
    if (e?.name === "TimeoutError") disconnected(e.message);
    else if (!current.signal.aborted) error(e);
  }).finally(() => clearTimeout(timer));
}
async function connect(data: Selection & { id: string }) {
  const current = ++connection;
  await stopped;
  if (connection !== current) return;
  connectionFailure = undefined;
  clientReady = new Promise(resolve => { resolveClient = resolve; });
  id = data.id;
  runtime.socket.url = socketUrl(id);
  document.body.classList.remove("static");
  void runtime.clientProvider!.ensureClient(new FileUri("/project/lean-toolchain"));
  // The selected offline model already has the native file URI. The reference
  // adopts it without changing the source editor, cursor or undo history.
  selection.abort(); selection = new AbortController();
  prepare(currentNode ?? data);
}
function socketUrl(session: string) {
  return `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${projectPath(`/api/lean/session/${encodeURIComponent(session)}/ws`)}`;
}
async function start() {
  document.body.classList.add("static");
  const infoview = document.getElementById("infoview")!;
  infoview.addEventListener('load', event => {
    if (!(event.target instanceof HTMLIFrameElement)) return;
    const scroller = event.target.contentDocument?.scrollingElement as HTMLElement | null;
    if (scroller) chainEditorScroll(scroller);
  }, true);
  runtime.setInfoviewElement(infoview);
  await initializeMonaco();
  await runtime.start({
    websocket: { url: socketUrl("") },
    // The parent replaces the session while retaining the editor. Retrying this consumed
    // socket inside vscode-languageclient races that recovery, including at init.
    clientOptions: {
      documentSelector: [{ language: "lean4" }],
      errorHandler: {
        error: () => ({ action: ErrorAction.Continue }),
        closed: () => {
          disconnected("Lean connection closed; check the service log for the native startup error.");
          return { action: CloseAction.DoNotRestart, handled: true };
        },
      },
    },
    htmlElement: document.getElementById("layout")!,
    vscode: {
      "editor.fontSize": 13,
      "editor.tabSize": 2,
      "editor.wordWrap": "on",
      "lean4.infoview.autoOpen": false,
      // JuliaMono is monospace. LeanMonaco's advanced default measures the
      // entire document in the DOM on every resize, blocking Safari's paint.
      "editor.wrappingStrategy": "simple",
      "workbench.colorTheme": params.get("theme") === "light" ? "Default Light Modern" : "Default Dark Modern",
    },
  });
  // Upstream's browser fallback treats each file's parent as a project. Our
  // session has one known root, including modules nested under Lib/EGA, etc.
  const provider = runtime.clientProvider!;
  const ensureClient = provider.ensureClient.bind(provider);
  provider.ensureClient = async () => {
    if (!id) return [false, undefined];
    return connecting ??= (async (): Promise<[boolean, LeanClient | undefined]> => {
      try {
        const result = await ensureClient(new FileUri("/project/lean-toolchain"));
        if (result[0] && result[1] && !result[1].isRunning()) await result[1].start();
        return result;
      } catch (e) {
        disconnected(`Lean failed to initialize: ${e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e)}`);
        return [false, undefined];
      } finally { connecting = undefined; }
    })();
  };
  runtime.clientProvider!.clientAdded((client: LeanClient) => {
    client.restarted(() => { if (id && client.isRunning()) resolveClient(client); });
    client.stopped((reason: { message: string }) => disconnected(reason.message));
    client.serverFailed((reason: string) => disconnected(reason));
    if (client.isRunning()) resolveClient(client);
    client.progressChanged(([uri, processing]: [string, { range: { start: { line: number } } }[]]) => {
      if (!id) return;
      const filename = Uri.parse(uri).path;
      const progress = progressText(processing);
      preparingProgress.set(filename, progress);
      const entry = documents.get(filename);
      if (!entry) return;
      entry.progress = progress;
      if (entry === selected) showProgress();
      if (processing.length === 0) void validate(entry);
    });
  });
  applyTheme(params.get("theme") ?? "light");
  preview = MonacoEditor.createModel("", "lean4", Uri.parse("inmemory://mdc/initial.lean"));
  // Resolve the native TextMate grammar before revealing any source. The public
  // colorizer awaits its lazy tokenizer; an empty input avoids tokenizing twice.
  await MonacoEditor.colorize("", "lean4", {});
  // Disable automatic layout at construction: changing the option later does
  // not disconnect Monaco's observer, leaving two competing layout callbacks.
  editor = MonacoEditor.create(document.getElementById("editor")!, {
    model: preview,
    ...sourceOptions,
    contextmenu: true,
    glyphMargin: true,
  });
  disposeScroll = nativeMonacoScroll(editor, document.getElementById("editor-scroll")!);
  editor.focus();
  editor.onDidChangeModelContent(() => { if (!updating) send("lean-change", editor.getValue()); });
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => send("lean-save"));
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => send("lean-save"));
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || event.source !== parent) return;
    if (event.data?.type === "lean-theme") applyTheme(event.data.value);
    if (event.data?.type === "lean-saved") {
      if (currentNode?.fnode === event.data.fnode) currentNode = {...currentNode, revision: event.data.revision};
      const entry = [...documents.values()].find(e => e.document.fnode === event.data.fnode);
      if (entry) {
        entry.document = { ...entry.document, source: event.data.source, revision: event.data.revision };
        void validate(entry);
      }
    }
    if (event.data?.type === "lean-starting") {
      document.body.classList.remove("static");
      document.getElementById("infoview-pending")!.hidden = false;
    }
    if (event.data?.type === "lean-start") void connect(event.data).catch(error);
    if (event.data?.type === "lean-stop") stopSession(event.data.session);
    if (event.data?.type === "lean-select" || event.data?.type === "lean-recheck") choose(event.data, event.data.type === "lean-recheck");
    if (event.data?.type === "lean-cancel") selection.abort();
    if (event.data?.type === "lean-source" && event.data.fnode === shownFnode && typeof event.data.value === "string" && editor.getValue() !== event.data.value) {
      editor.setValue(event.data.value);
    }
  });
  send("lean-runtime-ready");
}
void start().catch(error);
window.addEventListener("pagehide", () => {
  selection.abort();
  if (id) void fetch(projectPath(`/api/lean/session/${encodeURIComponent(id)}`), { method: "DELETE", keepalive: true }).catch(console.warn);
  preview?.dispose();
  disposeScroll?.();
  for (const entry of documents.values()) entry.reference.dispose();
  editor?.dispose(); runtime.dispose();
});
