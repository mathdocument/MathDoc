---
title: Databases, workspaces and references
---

## Databases and branches

A MathDoc project is a TerminusDB database. `mdc init NAME` (administrators)
creates the database `NAME` with a `main` branch. `mdc branch new NAME -p
DATABASE/BRANCH`, or **New branch** in the browser, forks a branch from another
branch's current commit; from then on each branch has its own commits, and
`mdc history` shows the latest 50 of one branch. Address a branch as
`DATABASE/BRANCH`, for example `myproject/main`; names use letters, digits, `_`
and `-`.

One API process serves every branch. A branch is loaded into memory on its first
request and stays loaded; there is no per-branch start or stop, no per-branch
port and no local compiler state. `mdc status` lists the branches you can read
with your role on each.

## Identity

Every API request carries `Authorization: Bearer TOKEN`. Tokens are configured in
`MDC_ACTORS` (see [Configuration](../../reference/configuration/)), a JSON object
mapping each actor name, such as `alice` or `agent-1`, to its token and
optionally `"admin": true`. Actor names use letters, digits, `_` and `-` (no
`/`); the server refuses to start otherwise. The actor name is recorded as the
author of document commits and collaboration commands. `GET /api/me` returns
`{actor, principal, admin, scopes}` for the current token.

### Delegated tokens

A person can give each agent process its own token without editing
`MDC_ACTORS`: `mdc token new NAME` (or `POST /api/tokens`) creates a **delegated
token** that acts as `PERSON/NAME`, for example `alice/codex-1`.

- Roles, project membership, ownership and administrator rights are the
  person's: `alice/codex-1` can read what alice can read and work on alice's
  projects.
- Leases, submissions, notes, reviews, events and commits record the full
  identity, so the work of two agents is told apart, and the task board adds up
  the budget by identity.
- Every delegated token can read what its person can read. **Scopes** limit the
  rest, whatever the person's roles:

  | Scope | Allows |
  | --- | --- |
  | `work` (default) | The work commands on project boards: `claim`, `heartbeat`, `finish`, `submit`, `resolve`, `note`, `negate`, `import` (a certificate), `submit_node`, `sync` and project `notes`. |
  | `maintain` | Owner and administrator actions: the owner's board commands (`members`, `task`, `decomposition`, `policy`, `pick`, `assemble`, `retry`), setting the proof environment, creating and removing databases, deleting branches, granting roles and `set_owner`. |
  | `edit` | Document writes, graph import, forking a branch, attaching a branch to the project, and `pick` on a branch the person may write (the owner can also pick with `maintain`). With `maintain`: creating the project board and accepting a writeback. |

  A refused action answers `403 token_scope_required` (document routes: `403`
  with the scope named).
- Only the person's own token creates, lists and revokes delegated tokens
  (`own_token_required` otherwise). Token names use letters, digits, `_` and
  `-`; a name stays taken until its token is revoked. The token is shown once;
  MathDoc keeps only its SHA-256. Revoking takes effect on the next request
  (`401`), and so does an `--expires` time. Removing the person from
  `MDC_ACTORS` disables their tokens.

## Workspaces and roles

Each database is a **workspace** with one owner and per-branch roles, stored in
PostgreSQL. Roles are `owner`, `editor` and `viewer`; administrators act as
`admin` everywhere.

| Action | Roles allowed |
| --- | --- |
| read, history, export | admin, owner, editor, viewer |
| write, branch create, proof submission (create or attach) | admin, owner, editor |
| branch delete, import, manage members (grant roles, set the proof environment) | admin, owner |
| database create, database delete | admin |

How a role is determined on a branch:

- The database's owner is `owner` on every branch that inherits the workspace
  role. `init` makes its caller the owner; `main` inherits.
- `mdc grant ACTOR ROLE -p DATABASE/BRANCH` gives an actor from `MDC_ACTORS` a
  role on that one branch; `none` removes it. When several apply, the highest
  wins.
- A new branch copies the rules of the branch it was forked from.
- Someone with no role on a branch gets `404 project not found`, as if it did not
  exist. A known branch with a role that is too low gets `403`.
- A database without a workspace record, for example one created by an older
  MathDoc version, is visible to administrators only. An administrator adopts it
  with the `set_owner` directory action ([HTTP API](../../reference/http-api/#directory)).

Collaboration never widens these permissions: every member of a database's
project must be able to read its `main` branch, and sees only the branches they
can read. Creating the project needs `owner` on `main`; attaching a branch needs
`editor` on that branch. See [Collaboration](../collaboration/#permissions).

## Where data lives

| Data | Location |
| --- | --- |
| Nodes, source blocks, edges, LaTeX settings, history | TerminusDB. |
| Workspace owners and roles, proof environments, project boards, tasks, attempts, events, jobs, delegated token hashes | PostgreSQL. |
| Certificates and proof sources | LeanGround; MathDoc stores their IDs. |
| Access tokens, database credentials | Server environment or a deployment's `secrets/`; never in exports. |
| Browser access token | The browser's `localStorage` for this site. |
| LaTeX runtime | `/opt/latex` in the Docker image; otherwise `MDC_CACHE_DIR/runtime/` when the backend installs it; regenerable. |
| Unsaved drafts | The browser tab; save before closing. |

A graph export contains one branch snapshot only; see
[Export and restore](../import-export/).

## Deleting

`mdc branch del -p DATABASE/BRANCH` deletes a branch other than `main` (owner or
admin). `main` can only go with the whole database: `mdc remove DATABASE` (admin)
deletes the database, every branch and its history, and the workspace record.
The database's proof environment and project boards stay in PostgreSQL. Both
commands act immediately, without a prompt (the browser asks first); unsaved
drafts in open tabs are lost.

## Node references

A node reference is its exact title or complete UUID. A duplicated title is
ambiguous and needs the UUID; UUID prefixes and paths are not references. Titles
can change; the UUID is stable. Nodes also keep a `module` field from the earlier
Lean integration; it is preserved in exports but not used for checking.
