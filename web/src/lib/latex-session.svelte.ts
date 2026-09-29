import { LatexSession as Session } from "./latex-session";
import { svelteModel } from "./svelte-model";
export * from "./latex-session";
export class LatexSession extends Session {
  constructor(...args: ConstructorParameters<typeof Session>) { super(...args); return svelteModel(this); }
}
