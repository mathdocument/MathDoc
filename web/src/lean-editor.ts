import { LeanMonaco, LeanMonacoEditor, type LeanClient } from "lean4monaco";
import { Uri, KeyCode, KeyMod, type editor as MonacoEditor } from "monaco-editor";
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
interface Document { fnode: string; filename: string; source: string; environment_key: string }
interface OpenDocument {
  document: Document;
  reference: LeanMonacoEditor["modelRef"];
  view: MonacoEditor.ICodeEditorViewState | null;
  progress: string;
}
const runtime = new BrowserLean();
const editor = new LeanMonacoEditor();
const documents = new Map<string, OpenDocument>();
let selected: OpenDocument | undefined;
let updating = false;
let selection = 0;
let selecting = Promise.resolve();
let resolveClient: (client: LeanClient) => void;
const clientReady = new Promise<LeanClient>(resolve => { resolveClient = resolve; });
const send = (type: string, value?: unknown, generation?: number) => parent.postMessage({ type, value, fnode: selected?.document.fnode, generation }, location.origin);
function showProgress() { send("lean-progress", selected?.progress ?? ""); }
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
  if (selected) selected.view = editor.editor.saveViewState();
  let entry = documents.get(next.filename);
  const restart = entry && entry.document.environment_key !== next.environment_key;
  if (!entry) {
    // ponytail: two hot documents; larger sets would retain another full Mathlib
    // environment per file. Evicted files keep Lake's shared compiled artifacts.
    if (documents.size >= 2) {
      const oldest = documents.keys().next().value!;
      if (documents.get(oldest) === selected) editor.editor.setModel(null);
      await evict(oldest);
    }
    const reference = await createModelReference(Uri.file(next.filename), next.source);
    entry = { document: next, reference, view: null, progress: "Loading Lean imports…" };
  }
  documents.delete(next.filename);
  documents.set(next.filename, entry);
  entry.document = next;
  selected = entry;
  updating = true;
  editor.editor.setModel(entry.reference.object.textEditorModel);
  if (editor.editor.getValue() !== next.source) editor.editor.setValue(next.source);
  if (entry.view) editor.editor.restoreViewState(entry.view);
  updating = false;
  editor.editor.focus();
  if (restart) { entry.progress = "Reloading changed Lean dependencies…"; runtime.clientProvider!.restartActiveFile(); }
  showProgress();
  send("lean-ready", undefined, generation);
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
    client.restarted(() => resolveClient(client));
    if (client.isRunning()) resolveClient(client);
    client.progressChanged(([uri, processing]: [string, { range: { start: { line: number } } }[]]) => {
      const entry = documents.get(Uri.parse(uri).path);
      if (!entry) return;
      entry.progress = processing.length === 0 ? "Lean editor ready" :
        processing.some(item => item.range.start.line === 0) ? "Loading Lean imports…" : "Lean is checking edits…";
      if (entry === selected) showProgress();
    });
  });
  applyTheme(params.get("theme") ?? "light");
  await editor.start(document.getElementById("editor")!, session.filename, session.source);
  selected = { document: session, reference: editor.modelRef, view: null, progress: "Loading Lean imports…" };
  documents.set(session.filename, selected);
  editor.editor.onDidChangeModelContent(() => { if (!updating) send("lean-change", editor.editor.getValue()); });
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => send("lean-save"));
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => send("lean-check"));
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || event.source !== parent) return;
    if (event.data?.type === "lean-theme") applyTheme(event.data.value);
    if (event.data?.type === "lean-select") {
      const { fnode, revision, generation } = event.data;
      const request = ++selection;
      selecting = selecting.then(() => selectNode(fnode, revision, generation, request)).catch(error);
    }
    if (event.data?.type === "lean-source" && event.data.fnode === selected?.document.fnode && typeof event.data.value === "string" && editor.editor.getValue() !== event.data.value) {
      editor.editor.setValue(event.data.value);
    }
  });
  new ResizeObserver(() => editor.editor.layout()).observe(document.getElementById("editor")!);
  send("lean-runtime-ready");
}
void start().catch(error);
window.addEventListener("pagehide", () => {
  if (id) void fetch(`/api/lean/session/${encodeURIComponent(id)}`, { method: "DELETE", keepalive: true }).catch(console.warn);
  for (const entry of documents.values()) entry.reference.dispose();
  editor.editor?.dispose(); runtime.dispose();
});
