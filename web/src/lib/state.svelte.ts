import { NodeSession as Session } from "./node-session";
import { svelteModel } from "./svelte-model";
export * from "./node-session";
export class NodeSession extends Session {
  constructor(...args: ConstructorParameters<typeof Session>) { super(...args); return svelteModel(this); }
}
export const nodeSession = new NodeSession();
