import { useEffect, useState } from "react";
import {
  confirmDiscardDraft,
  removeDraft,
  setDraftDirty,
} from "../lib/unsaved";
export function useDraft(
  name: string,
  dirty: boolean,
  revision: unknown = dirty,
) {
  const [id] = useState(() => Symbol(name));
  useEffect(() => {
    setDraftDirty(id, dirty);
  }, [id, dirty, revision]);
  useEffect(() => () => removeDraft(id), [id]);
  return {
    id,
    canClose: () => confirmDiscardDraft(id),
    clear: () => removeDraft(id),
  };
}
