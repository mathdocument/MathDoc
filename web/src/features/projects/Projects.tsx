import logo from "../../assets/mdc-logo.svg?no-inline";
import { useEffect, useRef, useState } from "react";
import {
  ArrowUpRight,
  FolderOpen,
  GitBranch,
  GitBranchPlus,
  Moon,
  Play,
  Plus,
  RefreshCw,
  Search,
  Square,
  Sun,
  Trash2,
} from "lucide-react";
import { projectsApi, isAbortError, type ServiceStatus } from "../../lib/api";
import { errMsg } from "../../lib/format";
import { useTheme } from "../../hooks/use-theme";
import { useLatest } from "../../hooks/use-latest";
import { IconButton } from "../../components/ui/button";
import { ErrorMessage } from "../../components/ui/dialog";
import { ProjectCreate } from "./ProjectCreate";

export default function Projects({ onReady }: { onReady?: () => void }) {
  const [status, setStatus] = useState<ServiceStatus | null>(null);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<"all" | "running" | "stopped">("all");
  const [create, setCreate] = useState<{ source?: string } | null>(null);
  const [pending, setPending] = useState<Record<string, string>>({});
  const [failures, setFailures] = useState<Record<string, string>>({});
  const { theme, toggleTheme } = useTheme();
  const lifetime = useRef<AbortController | null>(null);
  const refreshTask = useRef<Promise<void> | null>(null);
  const ready = useLatest(onReady);
  const branches = Object.entries(status?.projects ?? {}).map(
    ([name, state]) => {
      const [database, branch] = name.split("/");
      return { name, database: database!, branch: branch!, ...state };
    },
  );
  const running = branches.filter((row) => row.running).length;
  const projectCount = new Set(branches.map((row) => row.database)).size;
  const groups = new Map<string, typeof branches>();
  for (const row of branches) {
    if (
      !row.name.toLowerCase().includes(query.trim().toLowerCase()) ||
      (filter !== "all" && row.running !== (filter === "running"))
    )
      continue;
    const rows = groups.get(row.database) ?? [];
    rows.push(row);
    groups.set(row.database, rows);
  }
  async function load() {
    const controller = lifetime.current;
    if (!controller) return;
    setRefreshing(true);
    try {
      const result = await projectsApi.list(
        AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]),
      );
      if (controller.signal.aborted) return;
      setStatus(result);
      setError("");
    } catch (error) {
      if (!controller.signal.aborted && !isAbortError(error))
        setError(errMsg(error));
    } finally {
      if (!controller.signal.aborted) {
        setRefreshing(false);
        ready.current?.();
      }
    }
  }
  function refresh() {
    return (refreshTask.current ??= load().finally(() => {
      refreshTask.current = null;
    }));
  }
  async function changed() {
    await refreshTask.current;
    await refresh();
  }
  const actions = useLatest({ refresh });
  useEffect(() => {
    lifetime.current = new AbortController();
    document.title = "Projects · MathDoc";
    void actions.current.refresh();
    const check = () => {
      if (!document.hidden) void actions.current.refresh();
    };
    const timer = setInterval(check, 5000);
    document.addEventListener("visibilitychange", check);
    window.addEventListener("focus", check);
    return () => {
      clearInterval(timer);
      lifetime.current?.abort();
      document.removeEventListener("visibilitychange", check);
      window.removeEventListener("focus", check);
    };
  }, [actions]);
  function projectPending(database: string) {
    return (
      !!pending[database] ||
      branches.some((row) => row.database === database && !!pending[row.name])
    );
  }
  async function manage(
    project: string,
    action: "start" | "stop" | "delete_branch" | "remove",
  ) {
    const database = project.split("/")[0]!;
    if (
      pending[project] ||
      pending[database] ||
      (action === "remove" && projectPending(database))
    )
      return;
    if (
      action === "delete_branch" &&
      !confirm(`Delete ${project} and its Lean caches?`)
    )
      return;
    if (
      action === "remove" &&
      !confirm(
        `Delete entire project ${database}?\n\nAll its branches will be stopped. All project data, history, Lean caches and unsaved editor sessions will be deleted.`,
      )
    )
      return;
    setPending((previous) => ({ ...previous, [project]: action }));
    setFailures((previous) => ({ ...previous, [project]: "" }));
    try {
      await projectsApi.change(
        action === "remove" ? { action, database } : { action, project },
      );
    } catch (error) {
      setFailures((previous) => ({ ...previous, [project]: errMsg(error) }));
    } finally {
      await changed();
      setPending((previous) => {
        const next = { ...previous };
        delete next[project];
        return next;
      });
    }
  }
  return (
    <div className="directory">
      <header className="app-header">
        <a className="app-brand" href="/" aria-label="MathDoc projects">
          <img src={logo} alt="" />
          <strong>MathDoc</strong>
        </a>
        <div className="header-tools">
          <IconButton
            label="Refresh projects"
            variant="quiet"
            className={refreshing ? "spinning" : ""}
            disabled={refreshing}
            onClick={() => void refresh()}
          >
            <RefreshCw size={16} />
          </IconButton>
          <IconButton label="Toggle theme" variant="quiet" onClick={toggleTheme}>
            {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
          </IconButton>
        </div>
      </header>
      <main className="directory-main">
        <div className="directory-intro">
          <h1>Projects</h1>
          <div className="directory-summary" aria-label="Project summary">
            <span>
              <strong>{projectCount}</strong> projects
            </span>
            <span>
              <strong>{branches.length}</strong> branches
            </span>
            <span className="running-total">
              <i /> <strong>{running}</strong> running
            </span>
          </div>
        </div>
        <div className="directory-controls">
          <label className="search-field">
            <Search size={17} />
            <input
              aria-label="Search projects and branches"
              placeholder="Search projects or branches..."
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <div className="directory-actions">
            <IconButton
              label="Init project"
              variant="quiet"
              onClick={() => setCreate({})}
            >
              <Plus size={16} />
            </IconButton>
            <div className="segmented" aria-label="Filter branches">
              {(["all", "running", "stopped"] as const).map((value) => (
                <button
                  key={value}
                  aria-pressed={filter === value}
                  onClick={() => setFilter(value)}
                >
                  {value === "all"
                    ? "All branches"
                    : value === "running"
                      ? "Running"
                      : "Stopped"}
                </button>
              ))}
            </div>
          </div>
        </div>
        <div className="project-scroll">
          <ErrorMessage>
            {error &&
              `Could not refresh projects: ${error}${status ? ". Showing the last known states." : ""}`}
          </ErrorMessage>
          {Object.entries(failures)
            .filter(([name, message]) => !name.includes("/") && message)
            .map(([name, message]) => (
              <ErrorMessage key={name}>
                Could not delete project {name}: {message}
              </ErrorMessage>
            ))}
          {!status && !error ? (
            <div className="empty-state" role="status">
              Loading projects...
            </div>
          ) : status && !branches.length ? (
            <div className="empty-state">
              <FolderOpen size={32} />
              <h2>No projects yet</h2>
            </div>
          ) : status && !groups.size ? (
            <div className="empty-state">
              <Search size={28} />
              <h2>No matching branches</h2>
            </div>
          ) : (
            <div className="project-list">
              {Array.from(groups, ([database, rows]) => (
                <section
                  className="project"
                  aria-label={`Project ${database}`}
                  key={database}
                >
                  <header className="project-head">
                    <FolderOpen size={19} />
                    <h2>{database}</h2>
                    <span className="project-branches subtle">
                      {rows.length} {rows.length === 1 ? "branch" : "branches"}
                    </span>
                    <IconButton
                      label={`Delete project ${database}`}
                      variant="quiet"
                      disabled={projectPending(database)}
                      onClick={() => void manage(database, "remove")}
                    >
                      <Trash2 size={15} />
                    </IconButton>
                  </header>
                  {rows.map((row) => (
                    <div
                      className="branch-row"
                      data-project={row.name}
                      key={row.name}
                    >
                      <span className="branch-name">
                        <GitBranch size={16} />
                        {row.branch}
                      </span>
                      <span
                        className={`branch-state ${row.running ? "online" : ""}`}
                      >
                        <i />
                        {row.running ? "Running" : "Stopped"}
                      </span>
                      <span
                        className="branch-counts"
                        aria-label={`Graph size of ${row.name}`}
                      >
                        {row.running && row.nodes !== undefined && (
                          <span>
                            <strong>{row.nodes.toLocaleString()}</strong>{" "}
                            {row.nodes === 1 ? "node" : "nodes"}
                          </span>
                        )}
                        {row.running && row.edges !== undefined && (
                          <span>
                            <strong>{row.edges.toLocaleString()}</strong>{" "}
                            {row.edges === 1 ? "edge" : "edges"}
                          </span>
                        )}
                      </span>
                      <div
                        className="branch-actions"
                        aria-label={`Manage ${row.name}`}
                      >
                        <IconButton
                          label={`${row.running ? "Stop" : "Start"} ${row.name}`}
                          disabled={!!pending[row.name] || !!pending[database]}
                          onClick={() =>
                            void manage(row.name, row.running ? "stop" : "start")
                          }
                        >
                          {pending[row.name] ? (
                            <RefreshCw size={15} className="spinning" />
                          ) : row.running ? (
                            <Square size={13} />
                          ) : (
                            <Play size={14} />
                          )}
                        </IconButton>
                        <IconButton
                          label={`New branch from ${row.name}`}
                          disabled={!!pending[row.name] || !!pending[database]}
                          onClick={() => setCreate({ source: row.name })}
                        >
                          <GitBranchPlus size={16} />
                        </IconButton>
                        {row.branch === "main" ? (
                          <span className="branch-delete-space" />
                        ) : (
                          <IconButton
                            label={`Delete ${row.name}`}
                            disabled={
                              row.running ||
                              !!pending[row.name] ||
                              !!pending[database]
                            }
                            onClick={() => void manage(row.name, "delete_branch")}
                          >
                            <Trash2 size={15} />
                          </IconButton>
                        )}
                        {row.running &&
                        row.url &&
                        !pending[row.name] &&
                        !pending[database] ? (
                          <a
                            className="branch-open"
                            href={`/p/${row.name}/`}
                            aria-label={`Open ${row.name}`}
                          >
                            Open <ArrowUpRight size={16} />
                          </a>
                        ) : (
                          <span className="branch-open-space" />
                        )}
                      </div>
                      {failures[row.name] && (
                        <ErrorMessage>{failures[row.name]}</ErrorMessage>
                      )}
                    </div>
                  ))}
                </section>
              ))}
            </div>
          )}
        </div>
      </main>
      {create && (
        <ProjectCreate
          source={create.source}
          onClose={() => setCreate(null)}
          onCreated={async () => {
            setQuery("");
            setFilter("all");
            await changed();
          }}
        />
      )}
    </div>
  );
}
