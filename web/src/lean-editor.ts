import { LeanMonaco, LeanMonacoEditor, type LeanClient } from "lean4monaco";
import { Uri, KeyCode, KeyMod, editor as MonacoEditor } from "monaco-editor";
import { createModelReference } from "vscode/monaco";
import { FileUri } from "lean4monaco/dist/vscode-lean4/vscode-lean4/src/utils/exturi";

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
  reference: LeanMonacoEditor["modelRef"];
  view: MonacoEditor.ICodeEditorViewState | null;
  progress: string;
  validating?: boolean;
  validated?: string;
}
const runtime = new BrowserLean();
const editor = new LeanMonacoEditor();
const documents = new Map<string, OpenDocument>();
let selected: OpenDocument | undefined;
let shownFnode = "";
let preview: MonacoEditor.ITextModel | undefined;
let updating = false;
let selection = 0;
let selecting = Promise.resolve();
let resolveClient: (client: LeanClient) => void;
const clientReady = new Promise<LeanClient>(resolve => { resolveClient = resolve; });
const send = (type: string, value?: unknown, generation?: number) => parent.postMessage({ type, value, fnode: shownFnode, generation }, location.origin);
function showProgress() { send("lean-progress", preview ? "Preparing Lean environment…" : selected?.progress ?? ""); }
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
  if (selected && !preview) selected.view = editor.editor.saveViewState();
  const oldPreview = preview;
  selected = [...documents.values()].find(entry => entry.document.fnode === fnode);
  shownFnode = fnode;
  // In-memory models retain Lean highlighting but are excluded by the native
  // client's file/untitled selector. Typing need not wait for imports or LSP init.
  preview = selected ? undefined : MonacoEditor.createModel(source, "lean4", Uri.parse(`inmemory://mdc/${fnode}/${generation}.lean`));
  updating = true;
  editor.editor.setModel(selected?.reference.object.textEditorModel ?? preview!);
  if (editor.editor.getValue() !== source) editor.editor.setValue(source);
  if (selected?.view) editor.editor.restoreViewState(selected.view);
  updating = false;
  oldPreview?.dispose();
  editor.editor.focus();
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
async function selectNode(fnode: string, revision: string, generation: number, request: number) {
  const client = await clientReady;
  if (request !== selection) return;
  const next: Document = await client.sendRequest("mdc/selectNode", { fnode, revision });
  if (request !== selection) return;
  document.getElementById("error")!.textContent = "";

  let entry = documents.get(next.filename);
  const restart = entry && entry.document.environment_key !== next.environment_key;
  if (!entry) {
    // ponytail: two hot documents; larger sets would retain another full Mathlib
    // environment per file. Evicted files keep Lake's shared compiled artifacts.
    if (documents.size >= 2) {
      const oldest = documents.keys().next().value!;
      await evict(oldest);
      if (request !== selection) return;
    }
    const reference = await createModelReference(Uri.file(next.filename), next.source);
    if (request !== selection) {
      await reference.object.revert({ soft: true }); reference.dispose(); return;
    }
    entry = { document: next, reference, view: null, progress: "Loading Lean imports…" };
  }
  documents.delete(next.filename);
  documents.set(next.filename, entry);
  entry.document = next;
  const source = editor.editor.getValue();
  const view = editor.editor.saveViewState();
  const model = entry.reference.object.textEditorModel;
  if (!model) throw new Error("Lean document closed while switching nodes");
  selected = entry;
  updating = true;
  editor.editor.setModel(model);
  if (model.getValue() !== source) {
    model.pushStackElement();
    model.pushEditOperations([], [{ range: model.getFullModelRange(), text: source }], () => null);
    model.pushStackElement();
  }
  if (view) editor.editor.restoreViewState(view);
  updating = false;
  preview?.dispose(); preview = undefined;
  document.getElementById("infoview-pending")!.hidden = true;
  editor.editor.focus();
  if (restart) { entry.progress = "Reloading changed Lean dependencies…"; runtime.clientProvider!.restartActiveFile(); }
  showProgress();
  send("lean-ready", undefined, generation);
  void validate(entry);
}
async function start() {
  if (!id) throw new Error("Missing Lean session");
  const response = await fetch(`/api/lean/session/${encodeURIComponent(id)}`);
  const session: Document & { error?: string } = await response.json();
  if (!response.ok) throw new Error(session.error ?? "Lean session unavailable");
  runtime.setInfoviewElement(document.getElementById("infoview")!);
  await runtime.start({
    websocket: { url: `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/api/lean/session/${encodeURIComponent(id)}/ws` },
    htmlElement: document.getElementById("layout")!,
    vscode: {
      "editor.fontSize": 13,
      "editor.tabSize": 2,
      "editor.wordWrap": "on",
      "workbench.colorTheme": params.get("theme") === "light" ? "Default Light Modern" : "Default Dark Modern",
    },
  });
  // Upstream's browser fallback treats each file's parent as a project. Our
  // session has one known root, including modules nested under Lib/EGA, etc.
  const provider = runtime.clientProvider!;
  const ensureClient = provider.ensureClient.bind(provider);
  provider.ensureClient = () => ensureClient(new FileUri("/project/lean-toolchain"));
  runtime.clientProvider!.clientAdded((client: LeanClient) => {
    client.restarted(() => { if (client.isRunning()) resolveClient(client); });
    client.stopped((reason: { message: string }) => send("lean-disconnected", reason.message));
    client.serverFailed((reason: string) => send("lean-disconnected", reason));
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
  await editor.start(document.getElementById("editor")!, session.filename, session.source);
  shownFnode = session.fnode;
  selected = { document: session, reference: editor.modelRef, view: null, progress: "Loading Lean imports…" };
  documents.set(session.filename, selected);
  editor.editor.onDidChangeModelContent(() => { if (!updating) send("lean-change", editor.editor.getValue()); });
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => send("lean-save"));
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => send("lean-save"));
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
      const request = ++selection;
      showNode(fnode, source, generation);
      selecting = selecting.then(() => selectNode(fnode, revision, generation, request)).catch(e => {
        if (request === selection) error(e);
      });
    }
    if (event.data?.type === "lean-source" && event.data.fnode === shownFnode && typeof event.data.value === "string" && editor.editor.getValue() !== event.data.value) {
      editor.editor.setValue(event.data.value);
    }
  });
  new ResizeObserver(() => editor.editor.layout()).observe(document.getElementById("editor")!);
  send("lean-runtime-ready");
}
void start().catch(error);
window.addEventListener("pagehide", () => {
  if (id) void fetch(`/api/lean/session/${encodeURIComponent(id)}`, { method: "DELETE", keepalive: true }).catch(console.warn);
  preview?.dispose();
  for (const entry of documents.values()) entry.reference.dispose();
  editor.editor?.dispose(); runtime.dispose();
});
