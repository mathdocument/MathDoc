import Workspace from "./features/workspace/Workspace";
import { createRoot } from "react-dom/client";
import { flushSync } from "react-dom";
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
const ready = new Promise<void>((resolve) => {
  pageReady = resolve;
});
window.addEventListener("pagereveal", (event) => {
  const transition = event.viewTransition;
  if (!transition) return;
  document.documentElement.dataset.vtScope = "ready";
  void transition.ready.catch(() => {});
  void ready
    .then(settleEditorLayout)
    .catch(console.error)
    .finally(() => {
      transition.skipTransition();
      delete document.documentElement.dataset.vtScope;
    });
});
// index.html blocks the first paint on this module. Commit the shell before
// returning control so native page transitions never capture an empty header.
flushSync(() =>
  createRoot(target).render(
    <Tooltip.Provider>
      {projectName() ? (
        <Workspace onReady={pageReady} />
      ) : (
        <Projects onReady={pageReady} />
      )}
    </Tooltip.Provider>,
  ),
);
