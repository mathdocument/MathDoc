/** A small external-store boundary: domain sessions have no UI-framework imports. */
export class ObservableModel {
  private version = 0;
  private readonly listeners = new Set<() => void>();
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  readonly getSnapshot = () => this.version;
  protected notifyListeners() {
    this.version++;
    for (const listener of this.listeners) listener();
  }
  /** Observe top-level immutable values; replace arrays/records instead of mutating them. */
  protected observe(...keys: string[]) {
    for (const key of keys) {
      let value = Reflect.get(this, key);
      Object.defineProperty(this, key, {
        enumerable: true, configurable: true,
        get: () => value,
        set: next => {
          if (Object.is(value, next)) return;
          value = next;
          this.notifyListeners();
        },
      });
    }
  }
}
