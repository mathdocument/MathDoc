// The state behind the collaboration page (docs/mathdoc.md §4.8): the current branch's project
// board, refreshed every five seconds, and the commands people send, the same as agents'.
// The page's panels read and act through one shared instance.
import { api, projectsApi } from "./api";
import {
  activeGoals, branchRoute, branchView, branchesNeeding, coordination, currentBranch, goalName, nextStep,
  proofTree, readOnlyReason, refusal, routeCloses, solvedGoals, writtenBack,
  type Board, type CertificateDetail, type Command, type Fact, type Note, type WritebackPreview,
} from "./coordination";
import { errMsg } from "./format";

export type Section = "tree" | "review" | "tasks" | "assemble" | "project";
/** The inspector's tabs for a goal. */
export type InspectorTab = "overview" | "notes" | "certificates";
/** What the inspector shows. */
export type Selection = { kind: "goal"; goal: string } | { kind: "fact"; fact: string } | null;

export class Collab {
  readonly branch = currentBranch();
  boards = $state<Board[]>([]);
  boardId = $state<string | null>(null);
  board = $state<Board | null>(null);
  me = $state("");
  // A delegated token (`person/name`) acts for its person, within its scopes (§2.7.4).
  principal = $state("");
  scopes = $state<string[] | null>(null);
  // §2.7.7: editors of this branch pick its route too; the role comes from the directory.
  writesBranch = $state(false);
  // A refused command stays until the next command; a failed load clears on the next good load.
  refused = $state<string | null>(null);
  loadError = $state<string | null>(null);
  busy = $state(false);
  loaded = $state(false);
  jobs = $state<Awaited<ReturnType<typeof coordination.jobs>>>([]);
  // §2.7.5: tasks follow the server's recommendation order.
  ranking = $state(new Map<string, number>());
  // Certificates read from LeanGround: source and the definitions they use.
  details = $state<Record<string, CertificateDetail | "loading" | { error: string }>>({});
  // What accepting each assembly would write, loaded on request (§4.9).
  previews = $state<Record<string, WritebackPreview | "loading" | { error: string }>>({});
  /** The root node's text block: the problem in words, shown above the work. */
  rootText = $state<string | null>(null);
  #timer: ReturnType<typeof setInterval> | undefined;
  #alive = true;

  constructor(board: string | null) {
    this.boardId = board;
  }

  async start() {
    try {
      const who = await coordination.me();
      this.me = who.actor;
      this.principal = who.principal ?? who.actor;
      this.scopes = who.scopes ?? null;
      const role = this.branch ? (await projectsApi.list()).projects[`${this.branch.database}/${this.branch.branch}`]?.role : undefined;
      this.writesBranch = role === "admin" || role === "owner" || role === "editor";
    } catch { /* shown by refresh */ }
    await this.refresh();
    this.#timer = setInterval(() => void this.refresh(), 5000);
  }
  stop() {
    this.#alive = false;
    clearInterval(this.#timer);
  }

  async refresh() {
    if (!this.branch) return;
    try {
      this.boards = await coordination.projects(this.branch.database, this.branch.branch);
      this.boardId ??= this.boards[0]?.id ?? null;
      if (this.boardId) {
        const board = await coordination.project(this.boardId);
        this.jobs = await coordination.jobs(this.boardId);
        if (board.project && !board.project.superseded)
          this.ranking = new Map((await coordination.next(this.boardId)).candidates.map((c, i) => [c.task, i + 1]));
        const first = !this.board || this.board.id !== board.id;
        this.board = board;
        if (first) void this.loadRootText();
      }
      this.loadError = null;
    } catch (e) {
      if (this.#alive) this.loadError = errMsg(e);
    } finally {
      this.loaded = true;
    }
  }
  async loadRootText() {
    const node = this.board?.project?.root_node ?? this.board?.request?.node;
    if (!node) return;
    try {
      const view = await api.nodeView(node);
      this.rootText = view.node.blocks.find((b) => b.srctype === "text" && b.content.trim())?.content ?? null;
    } catch { this.rootText = null; }
  }
  selectBoard(id: string) {
    this.boardId = id;
    this.board = null;
    this.details = {};
    this.previews = {};
    void this.refresh();
  }

  async run(command: Command) {
    if (!this.board || this.busy || this.frozen) return;
    this.busy = true;
    this.refused = null;
    try { await coordination.command(this.board, command); }
    catch (e) { this.refused = refusal(e, command); }
    await this.refresh();
    this.busy = false;
  }
  async loadDetail(cert: string) {
    if (!this.board || this.details[cert]) return;
    this.details[cert] = "loading";
    try { this.details[cert] = await coordination.fact(this.board.id, cert); }
    catch (e) { this.details[cert] = { error: errMsg(e) }; }
  }
  async togglePreview(run: string) {
    if (!this.board) return;
    if (this.previews[run]) { delete this.previews[run]; return; }
    this.previews[run] = "loading";
    try { this.previews[run] = await coordination.preview(this.board.id, run); }
    catch (e) { this.previews[run] = { error: errMsg(e) }; }
  }
  review(fact: string, state: "active" | "retired", comment?: string) {
    const text = comment?.trim();
    return this.run({ type: "decomposition", certificate: fact, state, ...(text ? { comment: text } : {}) });
  }

  isOwner = $derived(this.board !== null && this.board.owner === this.principal && (this.scopes === null || this.scopes.includes("maintain")));
  /** As the server allows (coordinator/src/domain.ts): authors, and a person for their tokens. */
  ownsNote = (n: Note) => n.actor === this.me || (!this.me.includes("/") && n.actor.split("/")[0] === this.me);
  /** The owner (a token with `maintain`) also removes any note. */
  canDeleteNote = (n: Note) => this.ownsNote(n) || this.isOwner;
  canPick = $derived(this.isOwner || (this.board !== null && this.board.members.includes(this.principal) && this.writesBranch && (this.scopes === null || this.scopes.includes("edit"))));
  view = $derived(this.board && this.branch ? branchView(this.board, this.branch.branch) : null);
  frozen = $derived(this.board ? readOnlyReason(this.board) : null);
  /** Commands that act on a branch name the current one on project boards. */
  here = $derived(this.board?.project && this.branch ? { branch: this.branch.branch } : {});
  facts = $derived(this.board ? Object.values(this.board.facts) : []);
  proposed = $derived(this.facts.filter((f) => f.state === "proposed"));
  active = $derived(this.board ? activeGoals(this.board) : new Set<string>());
  solved = $derived(this.board ? solvedGoals(this.board) : new Set<string>());
  /** The certificates "Assemble a route" would use on this branch, as the coordinator chooses them. */
  onRoute = $derived((this.board && this.view ? branchRoute(this.board, this.view.picks) : null) ?? new Set<string>());
  tree = $derived(this.board ? proofTree(this.board) : { root: null, others: [] });
  step = $derived(this.board && this.branch ? nextStep(this.board, this.branch.branch, this.isOwner) : null);
  tasks = $derived(this.board ? Object.values(this.board.tasks).filter((t) => t.state !== "cancelled")
    .sort((x, y) => (this.ranking.get(x.id) ?? Infinity) - (this.ranking.get(y.id) ?? Infinity)) : []);
  openTasks = $derived(this.tasks.filter((t) => !this.solved.has(t.goal)));
  runs = $derived(this.board ? Object.values(this.board.runs).reverse() : []);
  /** This branch's certified assemblies not yet written back. */
  awaiting = $derived(this.board ? this.runs.filter((r) => r.status === "certified" && r.branch === this.branch?.branch && !writtenBack(this.board!, r)).length : 0);
  failures = $derived(this.jobs.filter((j) => j.status === "failed").slice(0, 5));
  assembleBlocked = $derived.by(() => {
    const b = this.board;
    if (!b || !b.root) return "There is no root goal yet.";
    if (!b.project) return this.statusOf(b.root) === "insufficient" ? "The root goal has no proof yet." : null;
    if (!this.view?.request.bound) return "This branch is not converted yet.";
    if (!routeCloses(b, this.view.picks)) return `Some goal on ${this.branch?.branch}'s route has no proof yet.`;
    return null;
  });

  name = (goal: string) => (this.board ? goalName(this.board, goal) : goal);
  statusOf = (goal: string) => this.board?.goals[goal]?.status ?? "insufficient";
  needing = (goal: string) => (this.board ? branchesNeeding(this.board, goal) : []);
  pickedBy = (fact: string) =>
    Object.entries(this.board?.branches ?? {}).filter(([, s]) => Object.values(s.picks).includes(fact)).map(([name]) => name);
  #runOf = (f: Fact) => Object.values(this.board?.runs ?? {}).find((r) => r.certification_id === f.id);
  #nodeOf = (f: Fact) => Object.values(this.board?.branches ?? {}).flatMap((s) => Object.values(s.nodes)).find((n) => n.certificate === f.id);
  /** Submitted by an actor (an agent or a person), not an assembly's result or a document node's. */
  submitted = (f: Fact) => !!f.submitter && !this.#runOf(f) && !this.#nodeOf(f);
  /** Where a certificate came from: an assembly's result, a document node, a submission, or LeanGround. */
  origin = (f: Fact) => {
    const b = this.board;
    if (!b) return "";
    const run = this.#runOf(f);
    if (run) return `result of assembly ${run.id.slice(0, 8)}${run.branch ? ` on ${run.branch}` : ""}`;
    const node = this.#nodeOf(f);
    if (node) return `from the document node ${node.title}`;
    if (f.submitter) return `${f.premises.length ? "split" : "proved"} by ${f.submitter}${f.submitted_at ? ` · ${when(f.submitted_at)}` : ""}`;
    return "found in LeanGround";
  };
  kindOf = (f: Fact) => (f.premises.length ? `decomposition ⇐ ${f.premises.map(this.name).join(", ")}` : "complete proof");
}

export const when = (ms: number | undefined) => (ms ? new Date(ms).toLocaleString() : "");
/** A delegated token acts as `person/name`: the person, through that token. */
export const actorName = (a: string) => (a.includes("/") ? `${a.slice(0, a.indexOf("/"))} (token ${a.slice(a.indexOf("/") + 1)})` : a);
