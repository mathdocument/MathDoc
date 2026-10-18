---
title: Web interface
---

## Sign in

The API process serves the editor at `/`. On load the page checks the stored
token against `/api/me`; without a valid one it asks for an **access token**,
which it then keeps in this browser's `localStorage`. Any later `401` response
clears it and returns to the sign-in page. A delegated token works too; the page
then acts as `PERSON/NAME` within the token's scopes. Both page headers show the
actor you are signed in as; **Sign out** next to it forgets the token in this
browser and returns to the sign-in page, for example to continue as someone
else. Use a private browser profile on shared machines.

## Project directory

The root page lists the branches you can read, grouped by database, with your
role on each and node/edge counts for branches already loaded by the server. It
refreshes every five seconds and filters by name. **Init project**
(administrators) and **New branch** (on any row) ask for a name; branch deletion
(not for `main`) and **Delete project** ask for confirmation. The server enforces
the role required for each. Branches load on demand, so there are no start or stop
controls. Workspace roles are managed with `mdc grant`; there is no role editor
in the browser.

## Editor

`/p/DATABASE/BRANCH/` opens the editor for one branch, at the node named by
`#ref=TITLE_OR_UUID` (with `&label=LABEL`, at that LaTeX label in Preview) or
else at the deepest root. The MathDoc logo returns to the directory; unsaved
drafts ask for confirmation.

| Toolbar control | Action |
| --- | --- |
| **Search nodes** (`/`) | Find a node by title or UUID. |
| **Back**, **Forward** | Move through the visited nodes (browser history). |
| **Create node** | A new node with a title only. |
| **Add dependency**, **Remove dependency** | Edit the current node's direct dependencies. |
| **Delete node** | Delete the node and every edge pointing to it, after confirmation. |
| **Knowledge**, **Graph** (`g`) | Switch between the column view and the graph view. |
| **Collaboration** | Opens the branch's collaboration page ([below](#collaboration-page)). |
| **Project settings** | Proof environment and LaTeX settings ([below](#lean-nodes-and-leanground)). |
| **Refresh database view** | Reload the current node and the branch's node and edge counts. |

The Knowledge view shows the node between its **Referrers** and
**Dependencies**; each entry shows its depth and Lean and Rocq lights (a
definition is tagged `def` and its light is a diamond), and each column has a
filter. A column with no entries shrinks to a narrow rail, leaving the width to
the editor. Click the node title to rename it. In the Graph view the editor takes
3/8 of the width and the graph 5/8. A graph small enough to order quickly (at most
450 boxes, counting a bend for each layer a long edge crosses) is drawn as the
collaboration page draws the proof graph: a card per node with its title, its
Lean standing as an icon and a word, each node above its dependencies, edges
orthogonal with rounded corners, ordered to cut crossings. The selected node's
card is outlined in the accent colour, its neighbours' cards darker, and its
edges coloured as the Referrers (up) and Dependencies (down) columns. Clicking a
card selects the node, clicking it again or the background clears the selection;
dragging the background pans, ⌘/Ctrl + wheel and the zoom buttons zoom, **Fit
graph** fits it to the view, and a node selected elsewhere scrolls into view.
Larger graphs are drawn on a canvas by depth, with the same colours and shapes.
Narrow screens stack graph and editor and hide the columns. Switching views keeps
unsaved edits. The status bar shows the current node and the branch's node and
edge counts.

All block types use Monaco editors with the same controls (**Save**, also
Ctrl/⌘+S; collapse; **Delete block**), fonts and light/dark themes; **Add source
block** adds a missing type. Lean blocks are plain source editors with syntax
highlighting. LaTeX blocks add Edit/Preview, reference and citation completion
and clickable cross-node references ([LaTeX](../latex/)). Text blocks are
GitHub-flavoured Markdown and add Edit/Preview too: math in `$…$`, `\(…\)`,
`$$…$$` or `\[…\]` is typeset with MathJax, raw HTML is sanitized, and links
open in a new tab. Text blocks open in Preview, except an empty one, which opens
in the editor. While editing, a Markdown toolbar inserts headings (H1–H3), bold
(⌘B), italic (⌘I), inline code, inline math `$…$`, display math `$$…$$`, a Lean
code block, bulleted and numbered lists, quotes and links; each button toggles
and is one undoable step, and typing `*`, `_`, `` ` `` or `$` over a selection
wraps it. The collaboration page's notes and review comments use the same
editor and toolbar. As for LaTeX, the preview shows the unsaved draft and switching
nodes keeps the mode. Editors do not mark characters that resemble ASCII, such
as Lean's `ℕ` or CJK punctuation; invisible characters stay marked.

## Node status

Each node shows a Lean and a Rocq status light. For Lean the light and label
come from LeanGround certification when the node has a Lean block:

| Label | Meaning |
| --- | --- |
| No code | No Lean block. |
| Not submitted to LeanGround | No conversion of the node on the database's project. |
| Insufficient evidence | Submitted; no accepted proof yet. |
| Derivable | Follows from certified results through conditional certificates, not yet assembled. |
| Certified | A certificate without open premises exists. |
| Changed since submission | The Lean text, or a definition it uses, changed after submission. |
| Not accepted | The converter or LeanGround rejected the node; the reason is shown, and hovering the light shows LeanGround's explanation (Lean's errors for a refused proof). |
| Registered definition | A definition node whose definitions LeanGround registered; definitions have no goal to prove. |

A node whose Lean block only defines (or that its conversion binds as a
definition) reads **Definition** until LeanGround registers it, then **Registered
definition**; it has no **Submit to LeanGround**, since a definition cannot be a
project's root, and is registered with the theorems that use it. The graph view
uses the same words, with the proof graph's icons: a hollow circle while nothing
is established (dashed without Lean code), a filled one once LeanGround has
(Derivable; Certified with a check), red for Changed since submission or Not
accepted, and a diamond for a definition (filled once registered). Its legend
counts each. Rocq is edited as source only: its light reads No code or
Unverified.

## Lean nodes and LeanGround

A Lean node's header has **Submit to LeanGround** while it is not submitted. It
asks for members and a budget (default 1000). In a database without a project it
creates one with this node as the root theorem (owners only); otherwise it
attaches the current branch to the project, converted from the root (you need
`editor` or better on the branch and membership in the project, and the branch
must contain the root node). Either way the database needs a proof environment.
Once the node has a status from the project, the button becomes
**Collaboration**.

**Project settings** has two tabs:

- **Proof environment**: LeanGround base key, context lines (`open`,
  `set_option`, `universe`, `variable`, one per line), Lean options as a JSON
  object, and minimum trust (`claimed`, `audited` by default, `trusted`), shared
  by all branches of the database. Owners only; a change makes the current
  project board read-only and the next submission starts a new one.
- **LaTeX**: the branch's shared macro file and bibliography.

## Collaboration page

The toolbar's **Collaboration** button, and the node header's **Collaboration**
button, open the branch's collaboration page at `/p/DATABASE/BRANCH/collab`.
It shows the database's project boards you are a member of, as seen from the
current branch, with a selector when there are several (superseded boards and
earlier per-branch proof requests). It refreshes every five seconds. Owner
controls appear for the board's owner (a delegated token also needs
`maintain`). A refused command shows its reason at the bottom of the page,
over every column, until the next command or **×**; a server older than the
page, which does not know a command, is named as such; a command sent while a background job changed the board is sent again at
the latest revision (up to three times), so a sync does not make a click fail.
The breadcrumb leads back to the document's root node; leaving the editor with
unsaved edits asks first. The address keeps the section, view, selected goal and
inspector tab (`?section=review`, `?view=graph`, `?goal=KEY&tab=notes`), so a
link opens the same place.

Below the page header are three columns, each scrolling on its own: the
sections (left), the problem with the current section (middle) and the
inspector (right). Drag the inspector's edge, or focus it and use the arrow
keys, to make it wider or narrower (300 pixels up to 60% of the window); the
width is kept in this browser. Another section starts at the top of the middle
column. Narrower than 1100 pixels, the inspector moves below and the page
scrolls as one.

**Problem.** At the top of the middle column: the project, owner and members; the root theorem's title and
status; the root node's text block as the problem (Markdown with typeset math,
in a serif face, folded to a few lines with **Show the whole problem**), or the root's Lean statement when there is
no text; and five **stages**: Decompose (the root is split or proved), Prove
(every goal on the branch's route proved, otherwise how many are open), Review
(no decomposition waiting), Assemble (the latest assembly certified) and Write
back (it is in the document). The first stage not done is marked current.

**Next step.** Above the section, one line says what the branch's proof waits
for and links the section that does it: an open goal to prove (the first few,
by name), a proposed decomposition to review, a route ready to assemble, a
running or failed assembly, a certified assembly to write back, or **Done** once
a certified assembly is written into the document (or the root is refuted).
Owners read "your review"; members read what waits for the owner.

**Sections** (left, with a count when something waits):

| Section | Contents |
| --- | --- |
| Proof tree | The certificates as a tree from the root, as an **Outline** or a **Graph**. Above the outline, a filter keeps the goals whose name contains the text and that are **Open**, have notes (**With notes**) or are **Claimed**, with the path from the root to them. The outline lists each goal with its status, who holds its task and for how long, and its number of notes (**open**, **provable by assembly** or **proved**; the API's `insufficient`, `derivable`, `certified`), under it its certificates (active first, complete proofs before decompositions; paused, retired and refuted ones fold under one line such as *4 retired*, which opens when one of them is selected) with origin, state when not active, **on BRANCH's route** for the certificates **Assemble a route** would use on the current branch (its picks, elsewhere the fewest premises first), the branches picking it, the last review and its ID, and under each decomposition its premises as goals; a goal reached twice is expanded once, under a certificate in use when one reaches it, and marked *shown above* where it recurs (*shown elsewhere* inside a fold); goals the root does not reach (such as negations) follow. The **Graph** draws the proof as an AND-OR graph. Each goal is one card, with a status icon (open, provable by assembly, proved) and a line saying who holds its task and for how long, how many notes it has and how many decompositions use it. Under a goal are its ways to prove it, as junctions: a check for a complete proof, a dot for a decomposition, with a cup and *all of N* when it needs several premises; under a decomposition, its premises. A goal several decompositions need is one card with several incoming edges, placed below all of them. Edges run orthogonally with rounded corners: a node's edges leave down one stem onto a bus below it, so a decomposition's premises hang from one bar; a long edge keeps one straight lane between the cards where it can, shared with its source's other long edges; edges into a goal land at separate points along its top; overlapping horizontal runs get separate tracks, stacked in the order that crosses least. Goals in a layer are ordered to cut crossings, and a lane avoids running through other decompositions' buses. A junction sits right under its goal. The branch's route (each goal's pick, else the way an assembly would take, else its first active way) is solid; every way it does not take is dashed, and goals off the route are faded. Above the graph, the route closes or has N open goals. Clicking a junction previews the route through that way: the solid lines and the count follow, and a bar says how many choices changed and whether the route then closes or which goals it leaves open; **Use on BRANCH** sends them as picks (the owner and members who can write the branch), **Reset** drops them. A way awaiting review, paused or retired is a hollow dashed dot that no route takes; those not in use are left out unless **Show ways not in use** is on. The graph fits the column's width until zoomed, and dragging its background pans it. Selecting a row or a box opens it in the inspector. |
| Review | When review is off, a notice says so (owners: **Require review**). Each decomposition awaiting review: its submitter and the branches that need it, its description (Markdown with `$…$` math), every premise with its Lean statement, the definitions it uses with their source (read from LeanGround) and its whole Lean source on request; owners **Activate** or **Retire** it with an optional comment. Then **Decided**, newest first: each decomposition the owner activated, paused or retired, with who, when and the comment, and each one an actor submitted while review was off, which counted at once. |
| Tasks | Open tasks, whose goal active certificates do not prove yet, in the server's recommended order, with goal (and the goal a negation refutes), kind, status (**inactive** when no active decomposition leads to the goal), who holds the claim and its time left, and the number of notes; done tasks folded away; budget by participant (a delegated token as its person and token name). Selecting a goal opens it in the inspector, where it is claimed. |
| Assemble & write back | What assembling and writing back do; owners **Assemble a route** for the current branch once its route is complete (otherwise why not yet), **Retry same plan** after a retryable failure and **Accept and write back** into the assembly's branch (not while it is stale); every assembly, newest first, with its route, a failed one with LeanGround's explanation, and its writebacks (what a committed one wrote, or why one aborted) instead of the button once written back; **Preview changes** shows what accepting writes, read against the branch as it is now (each node's Lean text as a line diff, and the assembled proof) or why the batch would abort, and writes nothing. |
| Project | Warnings for a read-only board, a stale or failed conversion of this branch, or a failed sync; **Attach this branch** when the branch is not attached; the document nodes the current branch converted, with kind, submission state and reason, goal status and **Resubmit** for theorem nodes, and what these words mean; with more than one branch, each branch's document state, picks and whether its route is complete; recent failed jobs with LeanGround's explanation; owner, members, budget, minimum trust, review policy (owners: **Require review** / **Waive review**) and last sync; **Sync now**. |

**Inspector** (right). For a goal: its status and what it means, then three
tabs, which stay as you select other goals. **Overview**: its Lean statement
(the type under the name and binders), the branches that need it, its task (kind
and place in the recommended order, holder and lease time left; **Claim** with
the budget to reserve and the lease length, **Extend** / **Release** for your
own claim, or why it cannot be claimed) and its latest note. **Notes**: a new
note (up to 500 characters, **Add note** or ⌘↵) and every note, newest first,
with its author, time and *edited* mark, rendered as Markdown with `$…$` math;
**Edit** (⌘↵ saves, Esc cancels) and **Delete** appear on notes you wrote, or a
delegated token of yours wrote, and **Delete** on every note for the owner.
**Certificates**: each as complete proof or decomposition with its number of
premises, state, route and origin. For a certificate (with a link back to its goal): what it proves
and from which premises (each with its statement), its origin, state, trust,
review and route, its description, the definitions it uses and its Lean source,
the review form while it is proposed, **Use on this branch** / **Unpick** (the
owner and members who can write the branch; a delegated token needs `edit`) and
the owner's **Activate** / **Pause** / **Retire**.

Read-only boards (a superseded project, or a per-branch proof request of an
earlier version) say why and disable their commands. Without a project, the
page says how to start one.

See [Collaboration](../collaboration/) for what these operations mean.
