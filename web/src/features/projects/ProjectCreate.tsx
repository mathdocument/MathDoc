import { useState, type FormEvent } from "react";
import { GitBranch } from "lucide-react";
import { projectsApi } from "../../lib/api";
import { errMsg } from "../../lib/format";
import { Dialog, ErrorMessage } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";

export function ProjectCreate({
  source,
  onCreated,
  onClose,
}: {
  source?: string;
  onCreated: (project: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const close = () => {
    if (!busy) onClose();
  };
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await projectsApi.change(
        source
          ? { action: "new_branch", project: source, name }
          : { action: "init", name },
      );
      if (!result.project)
        throw new Error("Project creation returned no branch");
      await onCreated(result.project);
      onClose();
    } catch (error) {
      setError(errMsg(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      title={source ? "New branch" : "Init project"}
      description={
        source
          ? `Create a branch from ${source}.`
          : "A new home for your mathematical work."
      }
      onClose={close}
      busy={busy}
    >
      <form onSubmit={submit}>
        <div className="dialog-body">
          <label className="field">
            {source ? "Branch name" : "Project name"}
            <input
              data-autofocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              required
              pattern="[A-Za-z0-9_\-]+"
              title="Letters, digits, hyphens and underscores"
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
            />
          </label>
          <div className="path-preview">
            <GitBranch size={15} />
            <code>
              {source
                ? `${source.split("/")[0]}/${name || "..."}`
                : `${name || "..."}/main`}
            </code>
          </div>
          <ErrorMessage>{error}</ErrorMessage>
        </div>
        <footer className="dialog-footer">
          <Button onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={busy || !name}>
            {busy ? "Creating..." : source ? "New branch" : "Init"}
          </Button>
        </footer>
      </form>
    </Dialog>
  );
}
