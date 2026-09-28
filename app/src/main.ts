import { mount } from "svelte";
import App from "./App.svelte";
import "./style.css";
import "katex/dist/katex.min.css";
mount(App, { target: document.getElementById("app")! });
