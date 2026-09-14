import { mount } from "svelte";
import App from "./App.svelte";
import { projectName } from "./lib/project-path";
import "./app.css";
import "./block.css";

const target = document.getElementById("app");
if (!target) {
  throw new Error("#app root element missing");
}

if (projectName()) mount(App, { target });
else void import("./Projects.svelte").then(({ default: Projects }) => mount(Projects, { target }));
