import { mount } from "svelte";
import App from "./App.svelte";
import Projects from "./Projects.svelte";
import { projectName } from "./lib/project-path";
import { accessToken } from "./lib/auth";
import SignIn from "./components/SignIn.svelte";
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
// Every API request carries an access token; without one, sign in first.
window.addEventListener("mdc:signed-out", () => location.reload());
if (accessToken()) mount(projectName() ? App : Projects, { target, props: { onReady: pageReady } });
else { mount(SignIn, { target }); pageReady(); }
