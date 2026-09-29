import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ChangeEvent,
  type RefObject,
} from "react";
import { Tabs } from "@base-ui/react/tabs";
import { api, type LeanProject } from "../../lib/api";
import { latexApi, type LatexProject } from "../../lib/latex";
import { errMsg } from "../../lib/format";
import { trackMutation } from "../../lib/unsaved";
import { useDraft } from "../../hooks/use-draft";
import { Dialog, ErrorMessage } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
type FormProps = {
  guard: RefObject<() => boolean>;
  onClose: () => void;
  onSaved: () => void;
};
function LeanSettings({ guard, onClose, onSaved }: FormProps) {
  const [project, setProject] = useState<LeanProject | null>(null),
    [revision, setRevision] = useState(""),
    [baseline, setBaseline] = useState("");
  const [busy, setBusy] = useState(true),
    [error, setError] = useState("");
  const draft = useDraft(
    "Lean environment",
    !!baseline && JSON.stringify(project) !== baseline,
    JSON.stringify(project),
  );
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    void api
      .project()
      .then((result) => {
        if (alive.current) {
          setProject(result.project);
          setRevision(result.revision);
          setBaseline(JSON.stringify(result.project));
        }
      })
      .catch((error) => {
        if (alive.current) setError(errMsg(error));
      })
      .finally(() => {
        if (alive.current) setBusy(false);
      });
    return () => {
      alive.current = false;
    };
  }, []);
  useLayoutEffect(() => {
    guard.current = () => !busy && draft.canClose();
  });
  async function save() {
    if (!project || busy) return;
    setBusy(true);
    setError("");
    const release = trackMutation();
    try {
      await api.putProject(project, revision);
      if (alive.current) {
        draft.clear();
        onSaved();
      }
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setBusy(false);
    }
  }
  return (
    <>
      <div className="dialog-body">
        <p className="subtle">
          Pin the toolchain and libraries here. Each environment keeps its own
          build cache. Reload open Lean editors after changing it.
        </p>
        <label className="field">
          Lean toolchain
          <input
            value={project?.toolchain ?? ""}
            disabled={busy}
            onChange={(event) =>
              setProject(
                (previous) =>
                  previous && { ...previous, toolchain: event.target.value },
              )
            }
          />
        </label>
        <label className="field">
          {project?.lakefile_name ?? "lakefile.toml"}
          <textarea
            value={project?.lakefile ?? ""}
            rows={9}
            spellCheck={false}
            disabled={busy}
            onChange={(event) =>
              setProject(
                (previous) =>
                  previous && { ...previous, lakefile: event.target.value },
              )
            }
          />
        </label>
        <label className="field">
          lake-manifest.json
          <textarea
            value={project?.manifest ?? ""}
            rows={7}
            spellCheck={false}
            disabled={busy}
            placeholder="Required for external libraries; paste the manifest produced by Lake."
            onChange={(event) =>
              setProject(
                (previous) =>
                  previous && {
                    ...previous,
                    manifest: event.target.value || null,
                  },
              )
            }
          />
        </label>
        <ErrorMessage>{error}</ErrorMessage>
      </div>
      <footer className="dialog-footer">
        <Button onClick={onClose} disabled={busy}>
          Cancel
        </Button>
        <Button
          variant="primary"
          onClick={() => void save()}
          disabled={busy || !revision}
        >
          Save environment
        </Button>
      </footer>
    </>
  );
}
function LatexSettings({ guard, onClose, onSaved }: FormProps) {
  const [project, setProject] = useState<LatexProject | null>(null),
    [revision, setRevision] = useState(""),
    [baseline, setBaseline] = useState("");
  const [busy, setBusy] = useState(true),
    [loadingFiles, setLoadingFiles] = useState(0),
    [error, setError] = useState("");
  const alive = useRef(true),
    draft = useDraft(
      "LaTeX environment",
      !!baseline && JSON.stringify(project) !== baseline,
      JSON.stringify(project),
    );
  useEffect(() => {
    alive.current = true;
    void latexApi
      .project()
      .then((result) => {
        if (alive.current) {
          setProject(result.project);
          setRevision(result.revision);
          setBaseline(JSON.stringify(result.project));
        }
      })
      .catch((error) => {
        if (alive.current) setError(errMsg(error));
      })
      .finally(() => {
        if (alive.current) setBusy(false);
      });
    return () => {
      alive.current = false;
    };
  }, []);
  useLayoutEffect(() => {
    guard.current = () => !busy && !loadingFiles && draft.canClose();
  });
  async function upload(
    event: ChangeEvent<HTMLInputElement>,
    kind: "preamble" | "bibliography",
  ) {
    const input = event.currentTarget,
      file = input.files?.[0];
    if (!file || !project) return;
    setError("");
    setLoadingFiles((value) => value + 1);
    try {
      if (file.size > (kind === "preamble" ? 2 : 16) * 1024 * 1024)
        throw new Error(`${kind} file is too large`);
      const content = await file.text();
      if (alive.current)
        setProject(
          (previous) =>
            previous && {
              ...previous,
              [kind]: content,
              [`${kind}_name`]: file.name,
            },
        );
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      if (alive.current) setLoadingFiles((value) => value - 1);
      input.value = "";
    }
  }
  async function save() {
    if (!project || busy || loadingFiles) return;
    setBusy(true);
    setError("");
    const release = trackMutation();
    try {
      await latexApi.putProject(project, revision);
      if (alive.current) {
        draft.clear();
        window.dispatchEvent(new Event("mdc-latex-project-changed"));
        onSaved();
      }
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setBusy(false);
    }
  }
  return (
    <>
      <div className="dialog-body">
        <p className="subtle">
          Share ordinary macros and bibliography across this branch. Uploaded
          contents are versioned with the graph.
        </p>
        <label className="field">
          Class or preamble
          <input
            type="file"
            aria-label="Class or preamble"
            accept=".cls,.tex"
            disabled={busy || !!loadingFiles}
            onChange={(event) => void upload(event, "preamble")}
          />
          {project && (
            <code className="subtle">
              {project.preamble_name} ·{" "}
              {project.preamble.length.toLocaleString()} characters
            </code>
          )}
        </label>
        <label className="field">
          Bibliography
          <input
            type="file"
            aria-label="Bibliography"
            accept=".bib"
            disabled={busy || !!loadingFiles}
            onChange={(event) => void upload(event, "bibliography")}
          />
          {project && (
            <code className="subtle">
              {project.bibliography_name} ·{" "}
              {project.bibliography.length.toLocaleString()} characters
            </code>
          )}
        </label>
        <p className="subtle">
          Node dependencies supply the available references automatically. Use
          Preview in each LaTeX block to view the result.
        </p>
        <ErrorMessage>{error}</ErrorMessage>
      </div>
      <footer className="dialog-footer">
        <Button onClick={onClose} disabled={busy || !!loadingFiles}>
          Cancel
        </Button>
        <Button
          variant="primary"
          onClick={() => void save()}
          disabled={busy || !!loadingFiles || !revision}
        >
          {busy ? "Preparing..." : "Save LaTeX project"}
        </Button>
      </footer>
    </>
  );
}
export function ProjectSettings({ onClose }: { onClose: () => void }) {
  const [tab, setTab] = useState("lean"),
    guard = useRef(() => true);
  const close = () => {
    if (guard.current()) onClose();
  };
  return (
    <Dialog
      title="Project settings"
      description="Versioned environments for this branch."
      onClose={close}
      wide
    >
      <Tabs.Root
        value={tab}
        onValueChange={(value) => {
          if (value !== tab && guard.current()) setTab(String(value));
        }}
      >
        <Tabs.List className="settings-tabs" aria-label="Project language">
          <Tabs.Tab value="lean">Lean</Tabs.Tab>
          <Tabs.Tab value="latex">LaTeX</Tabs.Tab>
        </Tabs.List>
        {tab === "lean" ? (
          <LeanSettings guard={guard} onClose={close} onSaved={onClose} />
        ) : (
          <LatexSettings guard={guard} onClose={close} onSaved={onClose} />
        )}
      </Tabs.Root>
    </Dialog>
  );
}
