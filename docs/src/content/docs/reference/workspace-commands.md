---
title: CLI commands
---

`mdc` is a client of the [HTTP API](../http-api/): each document and board
command is one or two requests made with your token, and the output is the JSON
response. Only two kinds of command keep local state, and neither ever holds a
token: the [proof work](#proof-work) commands, in a `.mdc/` directory next to their
goal files, and the board commands that [replay safely](#replaying-board-commands),
in a state directory.

## Setup

| Environment | Meaning |
| --- | --- |
| `MDC_URL` | Server origin. Unset, the [proof work](#proof-work) commands use the server their `.mdc/` workspace was created for, and every other command `http://127.0.0.1:17843`. |
| `MDC_TOKEN` | Your access token (required, except for `prove`/`split --dry-run`): your own, or a [delegated token](../../concepts/workspaces/#delegated-tokens). |
| `MDC_PROJECT` | Default `DATABASE/BRANCH` for branch, node, proof, goal and work commands. |
| `MDC_STATE_DIR` | Where `review`, `pick`, `branch new` and `proof command` keep their replay records; default `$XDG_STATE_HOME/mdc`, else `~/.local/state/mdc`. |

`-p DATABASE/BRANCH` selects the branch explicitly and overrides `MDC_PROJECT`.
Flags may appear anywhere after `mdc`. Short flags have long forms: `-p`
`--project`, `-t` `--target` (`new` also takes `--title`), `-n` `--max`, `-d`
`--depth`, `-m` `--message`, `-o` `--output`. From a checkout, run
`node coordinator/dist/cli.js` after `npm run build`; in the Docker image `mdc` is
on the `PATH`, and a deployment's `mdc` wrapper (`scripts/mdc-docker` in a
checkout) runs it from the host in a one-off container on the current directory
([Docker deployment](../../getting-started/server-deployment/#the-mdc-wrapper)).

Output is pretty-printed JSON on stdout. Exit code 2 means a usage error (the
usage text is printed on stderr); 1 means the command failed, with one line of
JSON on stderr: `{"reason": CODE, "status": HTTP_STATUS, "details": TEXT}`
(`status` only for HTTP errors; `"uncertain": true` when a sent request may have
taken effect). `mdc -h` prints the command list.

## Server and workspaces

| Command | Behavior |
| --- | --- |
| `mdc status` | `{actor, project, server, projects}`: who you are, your default branch (`-p` or `MDC_PROJECT`, `null` without one), and every branch you can read, with URL and your role. |
| `mdc init DATABASE` | Create a database with a `main` branch; you become its owner. Admin only. |
| `mdc remove DATABASE` | Delete the database, all branches and history, and its workspace record. Admin only; no prompt. |
| `mdc grant ACTOR owner\|editor\|viewer\|none -p DB/BRANCH` | Set ACTOR's role on that branch (`none` removes it). ACTOR must be a name in `MDC_ACTORS`. Owner or admin. |

Roles and their rights are listed in [Workspaces](../../concepts/workspaces/#workspaces-and-roles).

## Branches

| Command | Behavior |
| --- | --- |
| `mdc branch new NAME -p DB/BRANCH [--attach] [--pick GOAL=CERT]…` | Fork the selected branch's current head as `DB/NAME`; the new branch copies its role rules. Editor or better. With `--attach`, also attach it to the database's project and wait for its conversion; with `--pick` (repeatable, or comma-separated), attach it and then set its route at each GOAL to CERT, as `mdc pick` does. |
| `mdc branch del -p DB/BRANCH` | Delete a branch other than `main`. Owner or admin. |
| `mdc history -p DB/BRANCH` | The latest 50 TerminusDB commits. |
| `mdc export -p DB/BRANCH` | Whole-branch bundle ([Export and restore](../../concepts/import-export/)). |
| `mdc import FILE -p DB/BRANCH` | Load a bundle into an empty branch. Owner or admin. |
| `mdc project latex show -p DB/BRANCH` | `{revision, project}` with the shared macro file and bibliography. |
| `mdc project latex set --preamble FILE --bib FILE -p DB/BRANCH` | Upload both files (`.tex`/`.cls` up to 2 MiB and `.bib` up to 16 MiB), stored under their base names. Editor or better. |

There is no merge, rebase or checkout command; use TerminusDB directly for those.

## Graph

| Command | Behavior |
| --- | --- |
| `mdc graph check` | `{nodes, edges}`: the node and edge counts. |
| `mdc graph roots` | Unreferenced nodes with depth and component size, deepest first. |
| `mdc graph full` | All node summaries and index-pair edges. |
| `mdc search QUERY [-n N]` | Case-insensitive title/UUID matches, at most N (default and cap 200). |

See [Graph and metrics](../graph-and-metrics/).

## Nodes

References are exact titles or complete UUIDs.

| Command | Behavior |
| --- | --- |
| `mdc show REF` | The node with `revision`, blocks, dependencies and status. |
| `mdc new -t TITLE [--parent REF]` | Create and return a node; with `--parent`, also add the edge parent → new node in the same commit and return the updated parent (the new UUID is the last entry of its `depens`). |
| `mdc del REF` | Delete the node and every edge to it. |
| `mdc rename REF TITLE` | Change the title; the UUID stays. |
| `mdc edit REF [--type text\|lean\|rocq\|latex]` | Replace or create that block with stdin (default type `lean`). |
| `mdc edit REF --type TYPE --delete` | Delete the block; reads no stdin. |
| `mdc dep …`, `mdc metric ior REF` | See [Dependency commands](../dependency-commands/) and [Graph and metrics](../graph-and-metrics/). |

The guarded write commands, `new --parent`, `del`, `rename`, `edit`, `dep add`,
`dep rm` and `project latex set`, accept `--revision REV`: the revision printed by
`show` (for `new --parent` and `dep` the parent's or source's revision; for
`project latex set` the branch revision from `project latex show`). A stale
revision fails with `412` without writing. Without `--revision` the CLI reads the
current revision immediately before writing. `new` without `--parent`, `import`
and the branch commands take no revision.

```sh
mdc show -p myproject/main Lemma            # note "revision"
printf 'Explanation.\n' | mdc edit -p myproject/main Lemma --type text --revision REV
```

## Proofs

| Command | Behavior |
| --- | --- |
| `mdc proof env` | The database's proof environment (`404 environment_not_configured` if unset); `-p` names the database. |
| `mdc proof env set` | Set it from JSON on stdin; owner or admin. A change supersedes the current project board. |
| `mdc proof submit REF [--budget N] [--members a,b]` | Create the database's project rooted at REF, a node with a Lean block (owner of `main` or admin; budget default 1000), or attach the branch to the existing one (an editor of the branch who is a project member). |
| `mdc proof list [REF]` | The database's project boards you are a member of (current first), and earlier per-branch requests; with REF, those that converted it on this branch. |
| `mdc proof status ID` | The whole project board. |
| `mdc proof command ID` | Send one command (JSON on stdin): at the board's current revision, or with `If-Match: *` for the commands checked against the current state. Rerun after a lost reply, it takes effect once ([Replaying board commands](#replaying-board-commands)). |

```sh
echo '{"base_key": 1, "context": {"opens": ["Nat"]}}' | mdc proof env set -p myproject/main
mdc proof submit 'Main theorem' -p myproject/main --members bob,agent-1
echo '{"type": "sync"}' | mdc proof command PROJECT_ID
```

For proving, use the [proof work](#proof-work) commands; for reviewing and
choosing routes, the [goal and route](#goals-review-and-routes) commands. Other
agents can call the [HTTP API](../agents/) directly. Command types are listed in
[Proofs with LeanGround](../proofs/#commands).

## Goals, review and routes

These read the database's current project (`-p DATABASE/BRANCH` names it) and
send the owner's and editors' board commands.

| Command | Behavior |
| --- | --- |
| `mdc goals [--status S] [--branch B]` | One entry per goal, the root first: name, statement, status, `refuted`, the branches whose routes need it, certificates by state, its prove task and holder, and the numbers of notes and failed attempts. `--status` is `insufficient`, `derivable`, `certified` or `refuted`; `--branch` keeps the goals B's route reaches. |
| `mdc goal GOAL` | Everything about one goal: statement, declaration and definitions; its certificates with state, premises, description, submitter, last review and the branches picking them; the decompositions using it as a premise; each branch's pick; tasks with attempts; negation; notes. |
| `mdc review [--state S]` | The decompositions awaiting review (`proposed`, or state S: `active`, `paused`, `retired` or `refuted`), oldest first, with their goal, premises, description, submitter and the branches needing the goal. |
| `mdc review CERT --accept\|--pause\|--retire [-m COMMENT]` | Set the decomposition `active`, `paused` or `retired` with an optional comment (at most 2000 characters). Project owner only. |
| `mdc pick GOAL CERT\|auto [--branch B]` | Make B's route prove GOAL by CERT (a certificate of GOAL); `auto` lets the route choose again. The project owner, or a member who may write branch B (editor); a delegated token needs `maintain` as the owner or `edit` as an editor. |

CERT is a certificate ID or a unique prefix of at least 8 hex digits; for `pick`,
it must be a certificate of GOAL (`unknown_certificate` otherwise).

```sh
mdc review -p myproject/main                       # what awaits review
mdc review 4be1c09a -m "the right lemma" --accept  # activate it
mdc branch new by-commute -p myproject/main --pick 'Main theorem=7c2d90e1'
mdc goals -p myproject/main --branch by-commute    # the goals that route needs
```

### Replaying board commands

`review`, `pick`, `branch new` (its attach and picks) and `proof command` record
each board command's idempotency key and revision in the state directory before
sending it. If the reply is lost (`"uncertain": true` in the error), run the same
command again: it is resent with the same key and revision, and the server, which
checks the key first, returns the first result instead of applying it twice. If
the request never arrived and the board has moved on, the resend is refused as
outdated and the command is sent anew at the current revision. Once a reply has
arrived, running the same command again is a new command.

## Tokens

| Command | Behavior |
| --- | --- |
| `mdc token new NAME [--scope work,maintain,edit] [--expires 30d\|12h\|DATE]` | Create a delegated token acting as `YOU/NAME` (scope default `work`). Prints the token once. |
| `mdc token list` | Your delegated tokens with scopes, creation, expiry and revocation times; never the tokens. |
| `mdc token revoke NAME` | Revoke at once: the token's next request is `401`. |

These need your own token, not a delegated one. Give each agent process its
own token, so that leases, submissions and budgets are told apart:

```sh
export MDC_TOKEN=YOUR_TOKEN
mdc token new codex-1                  # {"token": "mdct_…", "actor": "you/codex-1", …}
MDC_TOKEN=mdct_… mdc pull double_eq -p myproject/main
```

## Proof work

Four commands take one goal of the database's current project from a claim to a
submitted proof or decomposition. They work on a **goal file**, a Lean file that
`mdc pull` writes, and keep its record in `.mdc/` (found in the current directory
or above, like `.git/`; `pull` creates it in the current directory when there is
none). A `.mdc/` belongs to one server: used with another `MDC_URL`, the commands
fail with `workspace_server_mismatch`.

| Command | Behavior |
| --- | --- |
| `mdc pull GOAL [--branch B] [-o FILE] [--ttl S] [--allocation N] [--with K1,K2] [--no-claim]` | Claim the goal's task (lease of S seconds, 30 to 3600, default 3600; N budget units reserved, default 0) and write its goal file, by default `<first 8 digits of the GoalKey>-<conclusion>.lean`. Never overwrites a file (`file_exists`). `--with` adds more goals as available premises; `--no-claim` writes the file without a lease. |
| `mdc prove FILE [--wait] [--dry-run] [--again]` | Submit the file as a complete proof (`has_premises` if a `sorry` premise remains). |
| `mdc split FILE -m DESCRIPTION [--wait] [--dry-run] [--again]` | Submit the file as a decomposition (`no_premises` if none remains), with DESCRIPTION (at most 2000 characters, Markdown with math in `$…$` or `$$…$$`) for its review. Its new premises become goals of the project, stated automatically. |
| `mdc check FILE` | Have LeanGround check the file now on the project's base, the same Lean and Mathlib that judge submissions: `ok`, `diagnostics` (`severity`, `line`, `col`, `text`), `axioms` and `trust`, with the diagnostics also printed as `FILE:LINE:COL: …` lines. Nothing is recorded and no certificate is made, so a check is no submission; it renews your leases like the work commands. A file `mdc pull` did not write is checked in the project of `-p` / `MDC_PROJECT`. |
| `mdc lemma QUERY... [--kind theorem\|definition\|all] [-n N]` | Library declarations of the project's base (Mathlib and results published to LeanGround) whose names contain one of the queries, exact and prefix matches first: `name`, `kind`, `type`, `trust` and the `matched_query`; at most N (default 20). Look a lemma up before citing it. |
| `mdc done [FILE] [--spent N] [--outcome O] [--note TEXT]` | End the lease, reporting `submitted` if the file was submitted under it and `no_progress` otherwise (or O: `submitted`, `no_progress`, `execution_failed` or `released`), with N budget units spent (default 0). Without FILE, the one file holding your lease. `--note` keeps what you learned on the goal (at most 500 characters, Markdown as for `mdc note`). |
| `mdc next [--branch B] [-n N] [--claim]` | The free goals most worth working on, best first, at most N (default 10) ([What to do next](../../concepts/collaboration/#what-to-do-next)); `--claim` pulls the first. |
| `mdc refute GOAL [pull flags]` | Register GOAL's negation if needed and pull it: proving that file with `mdc prove` refutes GOAL ([Refutation](../../concepts/collaboration/#refutation)). |
| `mdc note FILE\|GOAL -m TEXT` | Keep a note on the goal of a pulled file, or on GOAL (with `-p`): what was tried, where it got stuck. At most 500 characters of Markdown, with math in `$…$` (the web page typesets it; bare TeX such as `p^{k}` stays plain text); the next `pull` of the goal shows it. |

GOAL is a full GoalKey, a unique prefix of at least 8 hex digits, or the title or
UUID of a converted node on the branch (`-p DATABASE/BRANCH`, or `--branch`).
A goal without a node on the branch, such as the premise a split added, is also
found by the name `mdc goals` shows (`ambiguous_goal` when two goals have it).

```sh
mdc pull double_eq -p myproject/main      # writes 3f9a1c2e-double_eq.lean
# edit the proof below the line "-- mdc: edit below this line"
mdc split 3f9a1c2e-double_eq.lean -m "induction on n" --wait
mdc pull double_succ -p myproject/main    # the new subgoal, by name or key prefix
mdc prove 9d02b7c1-double_succ.lean --wait
mdc done 9d02b7c1-double_succ.lean
mdc done                                  # the remaining lease
```

**The goal file.** Above the line `-- mdc: edit below this line` is a read-only
header: the format line `-- mdc-goal-file/1`, the project, goal and conclusion
name, `import` of the base, the project's context and the registered definitions
the statements use, in the order LeanGround checks them, then the goal's notes as
of the pull. Changing it fails with `prefix_modified`. Below
the line are the available premises (`theorem … := sorry`, one per line) and the
goal's theorem, whose proof is `sorry`. You may:

- replace the goal's proof; its name and signature must stay
  (`root_missing`, `root_duplicated`, `root_statement_changed`);
- use the available premises by name; those you do not mention are dropped when
  you submit;
- add premises (`theorem name … := sorry`): each becomes a subgoal;
- add proved lemmas and definitions (`def`, `abbrev`, `structure`, …);
  definitions a premise statement mentions are registered with the submission.

The body may not contain `import` or context commands (`context_in_body`),
`example`, `instance`, `namespace`, `#` commands, attributes or `private`
(`unsupported_declaration`); continuation lines must be indented; names must be
unique (`duplicate_name`) and free of `'` (`invalid_name`); a premise may not
state the goal itself (`premise_equals_root`). `--dry-run` prints what would be
submitted without sending it, and needs neither the server nor a token.

**Replays.** Before sending, `mdc` records the submission's idempotency key and
the board revision. If the reply is lost, run the same command again: it resends
the same request, and the server returns the first result instead of running it
twice. If the request never arrived and the board has moved on, the resend is
refused as outdated and a new submission is made. The same file submitted again
reports the earlier submission (`replayed: true`); `--again` sends it anew.
`--wait` waits for the worker and reports the certificate, its
`certificate_state` and each new premise (`--timeout S`, default 600 seconds;
`refute` and `branch new --attach` wait for their jobs with the same limit).
A submission LeanGround refuses reports `"status": "rejected"`, the `reason`
(for example `verification_failed`) and `messages`: LeanGround's explanation,
for a proof Lean's errors with their positions in the submitted source. `mdc`
also prints the messages as plain text on standard error, to read them line by
line; fix the file and submit it again. A
decomposition is usually `proposed`: its new premises cannot be pulled with a
lease (`inactive_goal`) until the project owner activates it
([Review](../../concepts/collaboration/#review)); `--no-claim` still writes their
files.

**Leases.** Every `pull`, `prove`, `split`, `done` and `note` (not
`--dry-run`) also renews your leases in the workspace that are past half their
time. A lease you let lapse only stops
reserving the task for you: a correct proof submitted later is still recorded.
If someone else holds the task, `pull` names them; `--no-claim` lets you work on
it anyway.

### Module projects

In a [module project](../proofs/#module-projects) a goal is a directory of Lean files:

| Command | What it does |
| --- | --- |
| `mdc pull GOAL [-o DIR] [--ttl S] [--no-claim]` | Claim the goal and write DIR (default `<first 8 digits of the GoalKey>-<name>`): the goal's statement files to start from (the root's statement copied to `Work/Main.lean`; a premise's decomposition as it was submitted), the shared modules they import at their module paths and read-only, and `GOAL.md` with the goal, the rules and the latest notes. |
| `mdc check DIR [--root NAME]` | LeanGround compiles the directory's files and checks them as a contribution to the goal: `ok`, `compiled`, `premises`, `axioms`, and Lean's messages and the checker's findings as lines. Nothing is recorded. |
| `mdc prove DIR [--root NAME] [--wait]` | Submit every `.lean` file of DIR except the shared modules as a complete proof. With premises left, LeanGround refuses it before recording anything (`has_premises`). |
| `mdc split DIR -m TEXT [--shared FILE]… [--root NAME] [--wait]` | Submit them as a decomposition. Each `--shared FILE` holds definitions the new goals' statements use; those files are registered as a shared module first. |

The root theorem is the goal's statement constant unless `--root` names another one. A shared module
that was changed is refused (`shared_module_modified`), and so are paths that are not Lean identifiers
(`invalid_path`). `done`, `note`, leases and replays work as for goal files.

### MCP

`mdc mcp [--dir DIR]` serves `pull`, `prove`, `split`, `check`, `lemma`, `done` and `note` as tools
of a [Model Context Protocol](https://modelcontextprotocol.io) server over
stdio, for chat clients that call tools but have no shell or files. The tools
take and return the goal file's **text**: `pull` returns `{file, text, …}`;
`prove` and `split` take `{file, text}` (and `description` for `split`, `wait`
for both); `check` takes `{file, text}`; `lemma` takes `{queries, kind, limit}`;
`done` takes `{file, spent, outcome, note}` and `note` takes `{text}`
with `file` or `goal`. In a module project `pull` returns `{directory, files, …}` with the text of every
file of the directory, and `prove`, `split` and `check` take `file` (the directory) and `files` (path →
text, written before the call), with `root`, and `shared` (paths) for `split`.
The server keeps the files and their `.mdc/` records in DIR (default:
the current directory), so replays and leases work as on the command line. Run it
with a delegated token and the project:

```json
{"command": "mdc", "args": ["mcp", "--dir", "/path/to/work"],
 "env": {"MDC_URL": "https://mathdoc.example", "MDC_TOKEN": "mdct_…", "MDC_PROJECT": "myproject/main"}}
```

A refused tool call is a result with `isError: true` and the `{reason, …}` of the
failure.
