import { createSubscriber } from "svelte/reactivity";
import type { ObservableModel } from "./observable";

/** Temporary adapter while the React views replace the existing Svelte views. */
export function svelteModel<T extends ObservableModel>(model: T): T {
  const track = createSubscriber(update => model.subscribe(update));
  return new Proxy(model, { get(target, key, receiver) { track(); return Reflect.get(target, key, receiver); } });
}
