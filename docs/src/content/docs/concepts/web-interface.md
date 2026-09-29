---
title: Web interface
---

## Sign in

The API process serves the editor at `/`. Without a stored token the page asks
for an **access token**; it is checked against `/api/me` and kept in this
browser's `localStorage`. A `401` response clears it and returns to the sign-in
page. Use a private browser profile on shared machines.

## Project directory

The root page lists the branches you can read, grouped by database, with your
role on each and node/edge counts for branches already loaded by the server. It
offers **Init project** (administrators), **New branch** from any row, branch
deletion (not for `main`) and **Delete project**; the server enforces the role
required for each. Branches load on demand, so there are no start or stop
controls. Workspace roles are managed with `mdc grant`; there is no role editor
in the browser yet.

## Editor

`/p/DATABASE/BRANCH/` opens the editor for one branch. The MathDoc logo returns
to the directory; unsaved drafts ask for confirmation. Navigate by search,
dependency and referrer columns, or the graph view. Nodes are created with a
title only. In the graph view the editor takes 3/8 of the width and the graph
5/8; narrow screens stack them. Switching views keeps unsaved edits.

All block types use Monaco editors with the same controls, fonts and light/dark
themes. Lean blocks are plain source editors with syntax highlighting; there is
no Lean server or Infoview. LaTeX blocks add Edit/Preview, reference and citation
completion and clickable cross-node references ([LaTeX](../latex/)).

## Node status

Each node shows a Lean and a Rocq status light. For Lean the light and label
come from LeanGround certification when the node has a Lean block:

| Label | Meaning |
| --- | --- |
| No code | No Lean block. |
| Not submitted to LeanGround | Not part of any proof request. |
| Insufficient evidence | Submitted; no accepted proof yet. |
| Derivable | Follows from certified results through conditional certificates, not yet assembled. |
| Certified | A certificate without open premises exists. |
| Changed since submission | The Lean text, or a definition it uses, changed after submission. |
| Not accepted | The converter or LeanGround rejected the node; the reason is shown. |

The graph view colors nodes only by whether they have Lean code (gray: none,
yellow: Lean block present); certification is shown on node cards and in the
node header. Rocq is edited as source only.

## Lean nodes and LeanGround

A Lean node's header has **Submit to LeanGround**. It asks for members and a
budget and creates a proof request rooted at this node (you need `editor` or
better, and the branch needs a proof environment). Once submitted, the button
becomes **Collaboration**.

**Project settings** has two tabs:

- **Proof environment**: LeanGround base key, context lines (`open`,
  `set_option`, `universe`, `variable`), Lean options as JSON, and minimum trust
  (`claimed`, `audited`, `trusted`). Owners only; changes apply to new proof
  requests.
- **LaTeX**: the shared macro file and bibliography.

The toolbar's **Collaboration** dialog lists the branch's proof requests you are a
member of and has four views, refreshed every five seconds:

| View | Contents |
| --- | --- |
| Proof overview | Root status, owner, members, budget, trust, last sync; every converted node with role, state, reason and status; **Resubmit** and **Sync now**. |
| Task board | Tasks with lease holder and time left; **Claim**, **Extend**, **Release**; owners add decomposition tasks. |
| Decompositions | Certificates per goal with premises, trust and state; cycles and route membership marked; owners pause, retire or activate. |
| Review | Owners **Assemble a route**, **Retry same plan** and **Accept and write back**; writeback batches with their operations or abort reason. |

See [Collaboration](../collaboration/) for what these operations mean.
