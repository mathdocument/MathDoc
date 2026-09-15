import { LeanMonaco, type LeanClient } from "lean4monaco";
import { CancellationTokenSource, Uri, KeyCode, KeyMod, editor as MonacoEditor } from "monaco-editor";
import { createModelReference } from "vscode/monaco";
import { FileUri } from "lean4monaco/dist/vscode-lean4/vscode-lean4/src/utils/exturi";
import { CloseAction, ErrorAction } from "vscode-languageclient/lib/common/client.js";
import { projectPath } from "./lib/project-path";

const params = new URLSearchParams(location.search);
const id = params.get("session");
function applyTheme(theme: string) {
  document.body.style.backgroundColor = theme === "light" ? "#fff" : "#1f1f1f";
  runtime.updateVSCodeOptions({ "workbench.colorTheme": theme === "light" ? "Default Light Modern" : "Default Dark Modern" });
}
class BrowserLean extends LeanMonaco {
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
}
const runtime = new BrowserLean();
let editor: MonacoEditor.IStandaloneCodeEditor;
const documents = new Map<string, OpenDocument>();
let selected: OpenDocument | undefined;
let shownFnode = "";
let preview: MonacoEditor.ITextModel | undefined;
let updating = false;
let selection = new AbortController();
let connectionFailure: string | undefined;
let resolveClient: (client: LeanClient) => void;
const clientReady = new Promise<LeanClient>(resolve => { resolveClient = resolve; });
const send = (type: string, value?: unknown, generation?: number) => parent.postMessage({ type, value, fnode: shownFnode, generation }, location.origin);
function showProgress() { send("lean-progress", connectionFailure ? "" : preview ? "Preparing Lean environment…" : selected?.progress ?? ""); }
function disconnected(reason: string) {
  if (connectionFailure) return;
  connectionFailure = reason;
  selection.abort(new Error(reason));
  send("lean-disconnected", reason);
}
async function validate(entry: OpenDocument) {
  const key = () => `${entry.document.revision}:${entry.document.environment_key}:${entry.reference.object.textEditorModel?.getVersionId()}`;
  if (documents.get(entry.document.filename) !== entry || entry.validating || entry.validated === key() || entry.progress !== "Lean editor ready" ||
      entry.reference.object.textEditorModel?.getValue() !== entry.document.source) return;
  const requested = key(), { fnode, revision } = entry.document;
  const post = (type: string, value?: unknown) => parent.postMessage({ type, value, fnode }, location.origin);
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
function showNode(fnode: string, source: string, generation: number) {
  if (selected && !preview) selected.view = editor.saveViewState();
  const oldPreview = preview;
  selected = [...documents.values()].find(entry => entry.document.fnode === fnode);
  shownFnode = fnode;
  // In-memory models retain Lean highlighting but are excluded by the native
  // client's file/untitled selector. Typing need not wait for imports or LSP init.
  preview = selected ? undefined : MonacoEditor.createModel(source, "lean4", Uri.parse(`inmemory://mdc/${fnode}/${generation}.lean`));
  updating = true;
  editor.setModel(selected?.reference.object.textEditorModel ?? preview!);
  if (editor.getValue() !== source) editor.setValue(source);
  if (selected?.view) editor.restoreViewState(selected.view);
  updating = false;
  oldPreview?.dispose();
  editor.focus();
  document.getElementById("infoview-pending")!.hidden = !preview;
  showProgress();
  send("lean-ready", undefined, generation);
}
function error(error: unknown) {
  document.getElementById("error")!.textContent = String(error);
  send("lean-error", String(error));
}
async function evict(filename: string) {
  const old = documents.get(filename)!;
  documents.delete(filename);
  // Clear Monaco's dirty flag before releasing its reference, so VS Code closes
  // the document and Lean releases its worker. This never writes to the database.
  await old.reference.object.revert({ soft: true });
  old.reference.dispose();
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
async function selectNode(fnode: string, revision: string, generation: number, signal: AbortSignal) {
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
  const restart = entry && entry.document.environment_key !== next.environment_key;
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
  preview?.dispose(); preview = undefined;
  document.getElementById("infoview-pending")!.hidden = true;
  editor.focus();
  if (restart) { entry.progress = "Reloading changed Lean dependencies…"; runtime.clientProvider!.restartActiveFile(); }
  showProgress();
  send("lean-ready", undefined, generation);
  void validate(entry);
}
async function start() {
  if (!id) throw new Error("Missing Lean session");
  const response = await fetch(projectPath(`/api/lean/session/${encodeURIComponent(id)}`));
  const session: Document & { error?: string } = await response.json();
  if (!response.ok) throw new Error(session.error ?? "Lean session unavailable");
  runtime.setInfoviewElement(document.getElementById("infoview")!);
  await runtime.start({
    websocket: { url: `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${projectPath(`/api/lean/session/${encodeURIComponent(id)}/ws`)}` },
    // The parent recreates the session and restores drafts. Retrying this consumed
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
    try { return await ensureClient(new FileUri("/project/lean-toolchain")); }
    catch (e) {
      // lean4monaco's transport factory can reject before LeanClient installs
      // its failure listeners. Settle pending selections instead of timing out.
      disconnected(`Lean failed to initialize: ${e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e)}`);
      return [false, undefined];
    }
  };
  runtime.clientProvider!.clientAdded((client: LeanClient) => {
    client.restarted(() => { if (client.isRunning()) resolveClient(client); });
    client.stopped((reason: { message: string }) => disconnected(reason.message));
    client.serverFailed((reason: string) => disconnected(reason));
    if (client.isRunning()) resolveClient(client);
    client.progressChanged(([uri, processing]: [string, { range: { start: { line: number } } }[]]) => {
      const entry = documents.get(Uri.parse(uri).path);
      if (!entry) return;
      entry.progress = processing.length === 0 ? "Lean editor ready" :
        processing.some(item => item.range.start.line === 0) ? "Loading Lean imports…" : "Lean is checking edits…";
      if (entry === selected) showProgress();
      if (processing.length === 0) void validate(entry);
    });
  });
  applyTheme(params.get("theme") ?? "light");
  const reference = await createModelReference(Uri.parse(session.filename), session.source);
  // Disable automatic layout at construction: changing the option later does
  // not disconnect Monaco's observer, leaving two competing layout callbacks.
  editor = MonacoEditor.create(document.getElementById("editor")!, {
    model: reference.object.textEditorModel,
    automaticLayout: false,
    contextmenu: true,
    lineNumbersMinChars: 1,
    lineDecorationsWidth: 5,
  });
  editor.focus();
  shownFnode = session.fnode;
  selected = { document: session, reference, view: null, progress: "Loading Lean imports…" };
  documents.set(session.filename, selected);
  editor.onDidChangeModelContent(() => { if (!updating) send("lean-change", editor.getValue()); });
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => send("lean-save"));
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => send("lean-save"));
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || event.source !== parent) return;
    if (event.data?.type === "lean-theme") applyTheme(event.data.value);
    if (event.data?.type === "lean-saved") {
      const entry = [...documents.values()].find(e => e.document.fnode === event.data.fnode);
      if (entry) {
        entry.document = { ...entry.document, source: event.data.source, revision: event.data.revision };
        void validate(entry);
      }
    }
    if (event.data?.type === "lean-select") {
      const { fnode, revision, generation, source } = event.data;
      selection.abort();
      const current = selection = new AbortController();
      showNode(fnode, source, generation);
      if (connectionFailure) { error(connectionFailure); return; }
      const timer = setTimeout(() => current.abort(new DOMException("Lean node preparation timed out", "TimeoutError")), 30000);
      // Superseded requests cannot hold up the latest selection or replace its model.
      void abortable(selectNode(fnode, revision, generation, current.signal), current.signal).catch(e => {
        if (current !== selection) return;
        if (e?.name === "TimeoutError") send("lean-disconnected", e.message);
        else if (!current.signal.aborted) error(e);
      }).finally(() => clearTimeout(timer));
    }
    if (event.data?.type === "lean-cancel") selection.abort();
    if (event.data?.type === "lean-source" && event.data.fnode === shownFnode && typeof event.data.value === "string" && editor.getValue() !== event.data.value) {
      editor.setValue(event.data.value);
    }
  });
  new ResizeObserver(([entry]) => {
    // Hidden blocks have a zero viewport. Laying out there corrupts the saved
    // scroll position and can leave line-one diagnostics outside the visible area.
    if (entry.contentRect.width && entry.contentRect.height) editor.layout({ width: entry.contentRect.width, height: entry.contentRect.height });
  }).observe(document.getElementById("editor")!);
  send("lean-runtime-ready");
}
void start().catch(error);
window.addEventListener("pagehide", () => {
  selection.abort();
  if (id) void fetch(projectPath(`/api/lean/session/${encodeURIComponent(id)}`), { method: "DELETE", keepalive: true }).catch(console.warn);
  preview?.dispose();
  for (const entry of documents.values()) entry.reference.dispose();
  editor?.dispose(); runtime.dispose();
});
