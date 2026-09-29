---
title: Databases, workspaces and references
---

## Databases and branches

A MathDoc project is a TerminusDB database. `mdc init NAME` creates the database
`NAME` with a `main` branch. Branches of one database have independent graph
heads and share the database's commit history. Address a branch as
`DATABASE/BRANCH`, for example `myproject/main`; names use letters, digits, `_`
and `-`.

One API process serves every branch. A branch is loaded into memory on its first
request and stays loaded; there is no per-branch start or stop, no per-branch
port and no local compiler state. `mdc status` lists the branches you can read.

## Identity

Every API request carries `Authorization: Bearer TOKEN`. Tokens are configured in
`MDC_ACTORS` (see [Configuration](../../reference/configuration/)); each maps to an
actor name such as `alice` or `agent-1`, and optionally `admin: true`. The actor
name is recorded as the author of document commits and collaboration commands.
`GET /api/me` returns `{actor, admin}` for the current token.

## Workspaces and roles

Each database is a **workspace** with one owner and per-branch roles, stored in
PostgreSQL. Roles are `owner`, `editor` and `viewer`; administrators act as
`admin` everywhere.

| Action | Roles allowed |
| --- | --- |
| read, history, export | admin, owner, editor, viewer |
| write, branch create, proof request create | admin, owner, editor |
| branch delete, import, manage members (grant roles, set the proof environment) | admin, owner |
| database create, database delete | admin |

How a role is determined on a branch:

- The database's owner is `owner` on every branch that inherits the workspace
  role. `init` makes its caller the owner; `main` inherits.
- `mdc grant ACTOR ROLE -p DATABASE/BRANCH` gives a role on that one branch;
  `none` removes it. When several apply, the highest wins.
- A new branch copies the rules of the branch it was forked from.
- Someone with no role on a branch gets `404 project not found`, as if it did not
  exist. A known branch with a role that is too low gets `403`.
- A database without a workspace record, for example one created by an older
  MathDoc version, is visible to administrators only. An administrator adopts it
  with the `set_owner` directory action ([HTTP API](../../reference/http-api/#directory)).

Collaboration never widens these permissions: every member of a proof request
must be able to read its branch. See [Collaboration](../collaboration/#permissions).

## Where data lives

| Data | Location |
| --- | --- |
| Nodes, source blocks, edges, LaTeX settings, history | TerminusDB. |
| Workspace owners and roles, proof environments, proof requests, tasks, events, jobs | PostgreSQL. |
| Certificates and proof sources | LeanGround; MathDoc stores their IDs. |
| Access tokens, database credentials | Server environment or a deployment's `secrets/`; never in exports. |
| Browser access token | The browser's `localStorage` for this site. |
| LaTeX runtime | `MDC_CACHE_DIR/runtime/` when the backend installs it; regenerable. |
| Unsaved drafts | The browser tab; save before closing. |

A graph export contains one branch snapshot only; see
[Export and restore](../import-export/).

## Deleting

`mdc branch del -p DATABASE/BRANCH` deletes a branch other than `main` (owner or
admin). `main` can only go with the whole database: `mdc remove DATABASE` (admin)
deletes the database, every branch and its history, and the workspace record.
Both act immediately, without a prompt; unsaved drafts in open tabs are lost.

## Node references

A node reference is its exact title or complete UUID. A duplicated title is
ambiguous and needs the UUID; UUID prefixes and paths are not references. Titles
can change; the UUID is stable. Nodes also keep a `module` field from the earlier
Lean integration; it is preserved in exports but not used for checking.
