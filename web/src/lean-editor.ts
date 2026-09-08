import { LeanMonaco, LeanMonacoEditor } from "lean4monaco";
import { KeyCode, KeyMod } from "monaco-editor";

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
const runtime = new BrowserLean();
const editor = new LeanMonacoEditor();
const send = (type: string, value?: unknown) => parent.postMessage({ type, value }, location.origin);

async function start() {
  if (!id) throw new Error("Missing Lean session");
  const response = await fetch(`/api/lean/session/${encodeURIComponent(id)}`);
  const session = await response.json();
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
  applyTheme(params.get("theme") ?? "light");
  await editor.start(document.getElementById("editor")!, session.filename, session.source);
  editor.editor.updateOptions({ automaticLayout: true });
  editor.editor.onDidChangeModelContent(() => send("lean-change", editor.editor.getValue()));
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => send("lean-save"));
  editor.editor.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => send("lean-check"));
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || event.source !== parent) return;
    if (event.data?.type === "lean-theme") applyTheme(event.data.value);
    if (event.data?.type === "lean-source" && typeof event.data.value === "string" && editor.editor.getValue() !== event.data.value) {
      editor.editor.setValue(event.data.value);
    }
  });
  new ResizeObserver(() => editor.editor.layout()).observe(document.getElementById("editor")!);
  send("lean-ready");
}
void start().catch((error) => {
  document.getElementById("error")!.textContent = String(error);
  send("lean-error", String(error));
});
window.addEventListener("pagehide", () => {
  if (id) void fetch(`/api/lean/session/${encodeURIComponent(id)}`, { method: "DELETE", keepalive: true }).catch(console.warn);
  editor.dispose(); runtime.dispose();
});
