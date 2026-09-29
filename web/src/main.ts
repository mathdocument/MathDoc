import { mount } from "svelte";
import App from "./App.svelte";
import Projects from "./Projects.svelte";
import { projectName } from "./lib/project-path";
import { authorized } from "./lib/auth";
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
// Every request carries an access token; a 401 from /api/me shows the sign-in page.
window.addEventListener("mdc:signed-out", () => location.reload());
async function start() {
  let signIn = false;
  try {
    signIn = (await fetch("/api/me", authorized())).status === 401;
  } catch { /* unreachable server: the page reports its own API errors */ }
  if (signIn) { mount(SignIn, { target: target! }); pageReady(); }
  else mount(projectName() ? App : Projects, { target: target!, props: { onReady: pageReady } });
}
void start();
