import { WorkspaceSession as Session } from "./workspace-session";
import { svelteModel } from "./svelte-model";
export * from "./workspace-session";
export class WorkspaceSession extends Session {
  constructor(...args: ConstructorParameters<typeof Session>) { super(...args); return svelteModel(this); }
}
