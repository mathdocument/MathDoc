import { mount } from "svelte";
import App from "./App.svelte";
import Projects from "./Projects.svelte";
import { projectName } from "./lib/project-path";
import "./app.css";
import "./block.css";

const target = document.getElementById("app");
if (!target) {
  throw new Error("#app root element missing");
}

mount(projectName() ? App : Projects, { target });
