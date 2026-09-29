import { mount } from "svelte";
import App from "./App.svelte";
import { createRoot } from "react-dom/client";
import { Tooltip } from "@base-ui/react/tooltip";
import Projects from "./features/projects/Projects";
import "./ui.css";
import "./features/projects/projects.css";
import { projectName } from "./lib/project-path";
import { settleEditorLayout } from "./lib/editor-layout";
import "./app.css";
import "./block.css";

const target = document.getElementById("app");
if (!target) {
  throw new Error("#app root element missing");
}

let pageReady!: () => void;
const ready = new Promise<void>(resolve => { pageReady = resolve; });
window.addEventListener("pagereveal", event => {
  const transition = event.viewTransition;
  if (!transition) return;
  document.documentElement.dataset.vtScope = "ready";
  void transition.ready.catch(() => {});
  void ready.then(settleEditorLayout).catch(console.error).finally(() => {
    transition.skipTransition();
    delete document.documentElement.dataset.vtScope;
  });
});
if (projectName()) mount(App, { target, props: { onReady: pageReady } });
else createRoot(target).render(<Tooltip.Provider><Projects onReady={pageReady} /></Tooltip.Provider>);
