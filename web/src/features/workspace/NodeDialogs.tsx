import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from "react";
import { Search, Plus, ArrowUpRight, Check } from "lucide-react";
import { api, isAbortError } from "../../lib/api";
import { errMsg, shortFnode } from "../../lib/format";
import { nodeNameError } from "../../lib/node-name";
import type {
  DependencyCandidatesEmpty,
  NodeDetail,
  NodeInfo,
} from "../../lib/types";
import { confirmDiscardDrafts, trackMutation } from "../../lib/unsaved";
import { useDraft } from "../../hooks/use-draft";
import { Dialog, ErrorMessage } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";

export type MutationDelta = { nodes: number; edges: number };
export function NewNodeDialog({
  onCreated,
  onClose,
}: {
  onCreated: (id: string, skipGuard: boolean) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(""),
    [saving, setSaving] = useState(false),
    [error, setError] = useState("");
  const draft = useDraft("new node", !!name.trim(), name);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const close = () => {
    if (!saving && draft.canClose()) onClose();
  };
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (saving || nodeNameError(name) || !confirmDiscardDrafts(draft.id))
      return;
    setSaving(true);
    setError("");
    const release = trackMutation();
    try {
      const node = await api.newNode({ name });
      release();
      if (alive.current) {
        draft.clear();
        onCreated(node.fnode, true);
        onClose();
      }
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setSaving(false);
    }
  }
  return (
    <Dialog
      title="New node"
      label="new node"
      description="Give this idea a unique module name."
      onClose={close}
      busy={saving}
    >
      <form onSubmit={submit}>
        <div className="dialog-body">
          <label className="field">
            Name
            <input
              data-autofocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Example.Dev.Node"
              aria-invalid={!!name && !!nodeNameError(name)}
              aria-describedby="node-name-hint"
              autoComplete="off"
              disabled={saving}
            />
          </label>
          <small className="subtle" id="node-name-hint">
            {(name && nodeNameError(name)) ||
              "Unique module name, e.g. Example.Dev.Node. No spaces."}
          </small>
          <ErrorMessage>{error}</ErrorMessage>
        </div>
        <footer className="dialog-footer">
          <Button onClick={close} disabled={saving}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            disabled={saving || !!nodeNameError(name)}
          >
            {saving ? "Creating..." : "Create node"}
          </Button>
        </footer>
      </form>
    </Dialog>
  );
}

export function SearchDialog({
  target,
  onPick,
  onAdded,
  onClose,
}: {
  target?: NodeDetail;
  onPick: (id: string) => void;
  onAdded: (node: NodeDetail, delta: MutationDelta) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState(""),
    [results, setResults] = useState<NodeInfo[]>([]),
    [selected, setSelected] = useState(0);
  const [loading, setLoading] = useState(false),
    [saving, setSaving] = useState(false),
    [error, setError] = useState(""),
    [createMode, setCreateMode] = useState(false);
  const [empty, setEmpty] = useState<DependencyCandidatesEmpty | null>(null);
  const list = useRef<HTMLUListElement>(null),
    alive = useRef(true);
  const draft = useDraft("new dependency", createMode && !!query.trim(), query);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    setResults([]);
    setEmpty(null);
    setSelected(0);
    setError("");
    if (!query) {
      setLoading(false);
      return;
    }
    setLoading(true);
    const controller = new AbortController();
    const timer = setTimeout(async () => {
      try {
        const candidates = target
          ? await api.dependencyCandidates(
              target.fnode,
              query,
              50,
              controller.signal,
            )
          : {
              nodes: await api.search(query, 50, controller.signal),
              empty: null,
            };
        if (controller.signal.aborted) return;
        setEmpty(candidates.empty);
        setResults(candidates.nodes);
        setSelected(candidates.nodes.length ? 0 : -1);
      } catch (error) {
        if (!controller.signal.aborted && !isAbortError(error))
          setError(errMsg(error));
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    }, 120);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [query, target?.fnode]);
  const canCreate =
    !!target &&
    !nodeNameError(query) &&
    !results.length &&
    !loading &&
    empty?.kind === "no_match";
  const close = () => {
    if (!saving && draft.canClose()) onClose();
  };
  async function submit(index = selected) {
    if (saving) return;
    if (!target) {
      const node = results[index];
      if (node) onPick(node.fnode);
      return;
    }
    if (createMode ? !canCreate : !results[index]) return;
    setSaving(true);
    setError("");
    const release = trackMutation();
    try {
      const updated = createMode
        ? await api.newNode(
            { name: query.trim(), parent_fnode: target.fnode },
            target.revision,
          )
        : await api.addDep(
            target.fnode,
            results[index]!.fnode,
            target.revision,
          );
      release();
      if (!alive.current) return;
      draft.clear();
      onAdded(updated, { nodes: createMode ? 1 : 0, edges: 1 });
      onClose();
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setSaving(false);
    }
  }
  function key(event: KeyboardEvent) {
    if (saving || event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = Math.max(
        0,
        Math.min(
          results.length - 1,
          selected + (event.key === "ArrowDown" ? 1 : -1),
        ),
      );
      setSelected(next);
      list.current?.children[next]?.scrollIntoView({ block: "nearest" });
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (canCreate && !createMode) setCreateMode(true);
      else void submit();
    }
  }
  let message = query ? "No results" : "Search by name or UUID";
  if (empty?.kind === "excluded")
    message = !empty.source
      ? "all matches are already dependencies"
      : !empty.existing_dependencies
        ? "all matches refer to this node"
        : `matches excluded: ${empty.source} source, ${empty.existing_dependencies} existing`;
  return (
    <Dialog
      title={target ? "Add dependency" : "Find a node"}
      label={target ? "add dependency" : "search"}
      description={
        target
          ? `Connect a foundation to ${target.name}.`
          : "Move through the connected ideas in this project."
      }
      onClose={close}
      busy={saving}
    >
      <div className="dialog-body search-dialog-body">
        <label className="search-field">
          <Search size={17} />
          <input
            data-autofocus
            aria-label={target ? "Search dependencies" : "Search nodes"}
            placeholder="Search by name or UUID..."
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setCreateMode(false);
            }}
            onKeyDown={key}
            autoComplete="off"
            spellCheck={false}
            disabled={saving}
            role="combobox"
            aria-expanded={!!results.length}
            aria-controls="node-search-results"
            aria-activedescendant={
              results[selected] ? `node-search-${selected}` : undefined
            }
          />
        </label>
        <ErrorMessage>{error}</ErrorMessage>
        <ul
          ref={list}
          className="search-results"
          id="node-search-results"
          role="listbox"
          aria-label="Matching nodes"
        >
          {results.map((node, index) => (
            <li
              key={node.fnode}
              id={`node-search-${index}`}
              role="option"
              aria-selected={selected === index}
            >
              <button
                type="button"
                onMouseEnter={() => setSelected(index)}
                onClick={() => void submit(index)}
                disabled={saving}
              >
                <span>
                  {node.name}
                  <code>{shortFnode(node.fnode)}</code>
                </span>
                <ArrowUpRight size={14} />
              </button>
            </li>
          ))}
        </ul>
        {loading ? (
          <div className="search-empty" role="status">
            Searching...
          </div>
        ) : (
          !results.length && <div className="search-empty">{message}</div>
        )}
        {canCreate && (
          <div className="create-confirm">
            {createMode ? (
              <>
                <p>
                  Create <strong>{query}</strong> and add it as a dependency?
                </p>
                <Button
                  variant="primary"
                  disabled={saving}
                  onClick={() => void submit()}
                >
                  {saving ? "Creating..." : "Create and add"}
                </Button>
              </>
            ) : (
              <Button onClick={() => setCreateMode(true)}>
                <Plus size={14} />
                Create “{query}”
              </Button>
            )}
          </div>
        )}
      </div>
      <footer className="dialog-footer">
        <span className="shortcut-hint">
          <kbd>↑</kbd>
          <kbd>↓</kbd> navigate <kbd>Enter</kbd> select
        </span>
        <Button onClick={close} disabled={saving}>
          Cancel
        </Button>
      </footer>
    </Dialog>
  );
}

export function RemoveDependenciesDialog({
  target,
  children,
  onRemoved,
  onClose,
}: {
  target: NodeDetail;
  children: NodeInfo[];
  onRemoved: (node: NodeDetail, delta: MutationDelta) => void;
  onClose: () => void;
}) {
  const [selected, setSelected] = useState(new Set<string>()),
    [query, setQuery] = useState(""),
    [page, setPage] = useState(0),
    [cursor, setCursor] = useState(0);
  const [saving, setSaving] = useState(false),
    [error, setError] = useState("");
  const list = useRef<HTMLUListElement>(null),
    alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const matches = children.filter((child) =>
    `${child.name} ${child.fnode}`
      .toLowerCase()
      .includes(query.trim().toLowerCase()),
  );
  const pages = Math.max(1, Math.ceil(matches.length / 50)),
    visible = matches.slice(page * 50, (page + 1) * 50);
  const close = () => {
    if (!saving) onClose();
  };
  function toggle(id: string) {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }
  function changePage(value: number) {
    setPage(value);
    setCursor(0);
    if (list.current) list.current.scrollTop = 0;
  }
  async function submit() {
    const ids = children
      .filter((child) => selected.has(child.fnode))
      .map((child) => child.fnode);
    if (saving) return;
    if (!ids.length) {
      onClose();
      return;
    }
    setSaving(true);
    setError("");
    const release = trackMutation();
    try {
      const node = await api.rmDeps(target.fnode, ids, target.revision);
      release();
      if (alive.current) {
        onRemoved(node, { nodes: 0, edges: -ids.length });
        onClose();
      }
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setSaving(false);
    }
  }
  function key(event: KeyboardEvent) {
    if (
      saving ||
      event.nativeEvent.isComposing ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey
    )
      return;
    if (["ArrowDown", "ArrowUp", "j", "k"].includes(event.key)) {
      event.preventDefault();
      const next = Math.max(
        0,
        Math.min(
          visible.length - 1,
          cursor + (["ArrowDown", "j"].includes(event.key) ? 1 : -1),
        ),
      );
      setCursor(next);
      list.current?.children[next]?.scrollIntoView({ block: "nearest" });
    } else if (event.key === " " || event.key === "x") {
      event.preventDefault();
      if (visible[cursor]) toggle(visible[cursor]!.fnode);
    } else if (event.key === "Enter") {
      event.preventDefault();
      void submit();
    }
  }
  return (
    <Dialog
      title="Remove dependencies"
      label="remove dependencies"
      description="Select the connections to remove. The nodes themselves will remain."
      onClose={close}
      busy={saving}
    >
      <div className="dialog-body">
        <label className="search-field">
          <Search size={16} />
          <input
            data-autofocus
            type="search"
            aria-label="Filter dependencies"
            placeholder="Filter dependencies..."
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              changePage(0);
            }}
          />
        </label>
        <ul className="dependency-list" ref={list} onKeyDown={key}>
          {visible.map((node, index) => (
            <li key={node.fnode} className={cursor === index ? "current" : ""}>
              <label>
                <input
                  type="checkbox"
                  checked={selected.has(node.fnode)}
                  onChange={() => toggle(node.fnode)}
                  onFocus={() => setCursor(index)}
                  disabled={saving}
                />
                <span>
                  {node.name}
                  <code>{shortFnode(node.fnode)}</code>
                </span>
                {selected.has(node.fnode) && <Check size={14} />}
              </label>
            </li>
          ))}
        </ul>
        <div className="pagination">
          <Button
            aria-label="Previous"
            disabled={!page || saving}
            onClick={() => changePage(page - 1)}
          >
            Previous
          </Button>
          <span>
            Page {page + 1} / {pages} · {matches.length} dependencies
          </span>
          <Button
            aria-label="Next"
            disabled={page >= pages - 1 || saving}
            onClick={() => changePage(page + 1)}
          >
            Next
          </Button>
        </div>
        <ErrorMessage>{error}</ErrorMessage>
      </div>
      <footer className="dialog-footer">
        <span className="subtle" role="status">
          {selected.size} selected
        </span>
        <Button onClick={close} disabled={saving}>
          Cancel
        </Button>
        <Button
          variant="danger"
          onClick={() => void submit()}
          disabled={saving || !selected.size}
        >
          {saving ? "Removing..." : "Remove selected"}
        </Button>
      </footer>
    </Dialog>
  );
}
