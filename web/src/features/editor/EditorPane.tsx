import { useSessionLifetime } from "../../hooks/use-session-lifetime";
import {
  Fragment,
  useEffect,
  useLayoutEffect,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Check,
  FileText,
  Hash,
  Layers3,
  LoaderCircle,
  Save,
  X,
} from "lucide-react";
import { SOURCE_TYPES } from "../../lib/types";
import { nodeNameError } from "../../lib/node-name";
import { shortFnode } from "../../lib/format";
import { hasPendingMutations, subscribeMutations } from "../../lib/unsaved";
import { useModel } from "../../hooks/use-model";
import { FormalStatus } from "../../components/FormalStatus";
import { Button } from "../../components/ui/button";
import { EditorSession, type EditorSessionProps } from "./editor-session";
import { AddBlock } from "./AddBlock";
import { LeanBlock } from "./LeanBlock";
import "./editor-surfaces.css";
import "./editor.css";

export default function EditorPane(props: EditorSessionProps) {
  useSyncExternalStore(
    subscribeMutations,
    hasPendingMutations,
    hasPendingMutations,
  );
  const [session] = useState(() => new EditorSession(props));
  useSessionLifetime(session);
  const s = useModel(session),
    { node, BlockEditorComponent } = s;
  useLayoutEffect(() => {
    s.update(props);
  });
  useEffect(() => {
    window.addEventListener("keydown", s.saveShortcut);
    return () => {
      window.removeEventListener("keydown", s.saveShortcut);
    };
  }, [s]);
  const dirty = Object.keys(s.changes).length > 0;
  return (
    <section className="center" aria-label="current node">
      {props.load.kind === "idle" ? (
        <div className="empty-state">
          <FileText size={28} />
          <strong>No node selected</strong>
        </div>
      ) : props.load.kind === "error" ? (
        <div className="empty-state error-message">{props.load.message}</div>
      ) : (
        node && (
          <header className="node-head">
            <div className="node-heading">
              <div className="title-row">
                {s.editingTitle ? (
                  <>
                    <input
                      className="title-input"
                      aria-label="Node name"
                      aria-invalid={!!nodeNameError(s.titleDraft)}
                      ref={s.attachTitle}
                      value={s.titleDraft}
                      onChange={(event) => {
                        s.titleDraft = event.target.value;
                      }}
                      onKeyDown={(event) => {
                        if (event.nativeEvent.isComposing) return;
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void s.saveTitle();
                        } else if (event.key === "Escape") {
                          event.preventDefault();
                          s.cancelEditTitle();
                        }
                      }}
                      disabled={s.titleSaving}
                    />
                    <Button
                      className="icon-button"
                      onClick={() => void s.saveTitle()}
                      disabled={s.titleSaving || !!nodeNameError(s.titleDraft)}
                      aria-label="Save name"
                    >
                      <Check size={15} />
                    </Button>
                    <Button
                      className="icon-button"
                      onClick={s.cancelEditTitle}
                      disabled={s.titleSaving}
                      aria-label="Cancel rename"
                    >
                      <X size={15} />
                    </Button>
                  </>
                ) : (
                  <h1 className="title">
                    <button onClick={s.startEditTitle} title="Click to rename">
                      {node.name}
                    </button>
                  </h1>
                )}
              </div>
              {(s.titleError ||
                (s.editingTitle && nodeNameError(s.titleDraft))) && (
                <span className="title-error">
                  {s.titleError || nodeNameError(s.titleDraft)}
                </span>
              )}
            </div>
            <div className="node-meta" aria-label="node metadata">
              <code className="meta-item" title={node.fnode}>
                <Hash size={11} />
                {shortFnode(node.fnode)}
              </code>
              <span className="meta-item">
                <Layers3 size={12} />
                Depth {node.depth}
              </span>
              <span className="meta-sep" />
              <FormalStatus language="Lean" status={node.formalization.lean} />
              <FormalStatus language="Rocq" status={node.formalization.rocq} />
              <span className="node-save-controls">
                {dirty && (
                  <span className="node-unsaved" role="status">Unsaved</span>
                )}
                <Button
                  variant="quiet"
                  className={`node-save icon-button ${s.saving ? "spinning" : ""}`}
                  onClick={() => void s.saveNode()}
                  disabled={s.saving || hasPendingMutations() || !dirty}
                  aria-busy={s.saving}
                  aria-label="Save node"
                  title="Save all changed source blocks (Ctrl/⌘+S)"
                >
                  {s.saving ? <LoaderCircle size={15} /> : <Save size={15} />}
                </Button>
              </span>
            </div>
            {s.saveError && (
              <div className="error-message" role="alert">
                {s.saveError}
              </div>
            )}
          </header>
        )
      )}
      <div className={`blocks ${!node ? "hidden" : ""}`}>
        {node && !node.blocks.length && (
          <div className="empty-state">
            <FileText size={24} />
            <strong>No source blocks yet</strong>
            <p>Add a proof, a definition, or a written explanation.</p>
          </div>
        )}
        {s.editorLoadError ? (
          <div className="error-message" role="alert">
            Editor failed to load: {s.editorLoadError}
            <Button onClick={s.ensureBlockEditorLoaded}>retry</Button>
          </div>
        ) : (
          !BlockEditorComponent &&
          node?.blocks.some((block) => block.srctype !== "lean") && (
            <div className="editor-loading" aria-busy="true">
              Loading editor...
            </div>
          )
        )}
        {SOURCE_TYPES.map((srctype) => {
          const block = node?.blocks.find((block) => block.srctype === srctype);
          return (
            <Fragment key={srctype}>
              {srctype === "lean" ? (
                <LeanBlock
                  fnode={node?.fnode ?? ""}
                  revision={node?.revision ?? ""}
                  module={node?.name}
                  block={block}
                  theme={props.theme}
                  active={props.active}
                  selection={props.selection}
                  saving={s.saving}
                  onChange={(content) =>
                    s.updateDraft(node?.fnode ?? "", "lean", content)
                  }
                  onSave={s.saveNode}
                  onDeleted={(updated) => s.applyBlockUpdate(updated, true)}
                  onCertified={(updated) => s.applyBlockUpdate(updated, true)}
                  onReady={() => s.reportBlockReady("lean")}
                />
              ) : (
                node &&
                block &&
                BlockEditorComponent &&
                !s.editorLoadError && (
                  <BlockEditorComponent
                    key={`${node.fnode}:${srctype}`}
                    fnode={node.fnode}
                    revision={node.revision}
                    block={block}
                    theme={props.theme}
                    active={props.active}
                    saving={s.saving}
                    onChange={(content) =>
                      s.updateDraft(node.fnode, srctype, content)
                    }
                    onSave={s.saveNode}
                    latexPreview={props.latexPreview}
                    onPreviewChange={props.onPreviewChange}
                    preparedLatex={
                      props.load.kind === "ready"
                        ? props.load.latexPreview
                        : undefined
                    }
                    focusLabel={
                      props.latexTarget?.fnode === node.fnode
                        ? props.latexTarget.label
                        : undefined
                    }
                    onLatexNavigate={props.onLatexNavigate}
                    onDeleted={(updated) => s.applyBlockUpdate(updated)}
                    onReady={() => s.reportBlockReady(srctype)}
                  />
                )
              )}
            </Fragment>
          );
        })}
        {node && (
          <AddBlock
            key={node.fnode}
            node={node}
            onAdded={(updated) => s.applyBlockUpdate(updated, true)}
          />
        )}
      </div>
    </section>
  );
}
