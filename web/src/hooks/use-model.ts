import { useSyncExternalStore } from 'react';
import type { ObservableModel } from '../lib/observable';

export function useModel<T extends ObservableModel>(model: T): T {
  useSyncExternalStore(model.subscribe, model.getSnapshot, model.getSnapshot);
  return model;
}
