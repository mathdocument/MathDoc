---
title: Collaboration
---

A database's **project** lets several people and agents prove one theorem
together. Everyone uses the same board commands, whether through the browser's
**Collaboration** page, the
[`mdc` commands](../../reference/workspace-commands/#proof-work) or the
[HTTP API](../../reference/agents/). LeanGround checks every proof; MathDoc keeps
track of goals, tasks, budgets, notes, reviews, which decompositions each branch
uses, and what is written back.

## Projects and branches

A project belongs to one database and proves one theorem, its **root**. Its
**project board** holds everything the participants share: the goals, every
certificate LeanGround accepted for them (complete proofs and decompositions,
only ever added), tasks, leases, notes and the budget. Each branch of the
database is a different way to the same root: a **route** through the shared
goals, together with the branch's own document (statements, explanations,
LaTeX).

A project is fixed by:

- the database's **proof environment**: LeanGround base, context, options and
  minimum trust ([Proofs](../../reference/proofs/#proof-environment));
- the **root node**, the theorem node chosen when the project was created;
- the **owner** (the creator), **members** and a **budget**.

**Submit to LeanGround**, `mdc proof submit REF` or `POST
/api/coordination/projects` on a branch does one of two things:

- If the database has no project in its current environment, it creates one
  rooted at that node. Only the database owner (or an administrator) can do
  this, because it fixes the theorem for every branch.
- Otherwise it **attaches** the branch: any project member who is an editor of
  the branch can. The branch must contain the root node; every branch is
  converted from it, whichever node the button was pressed on.
  `mdc branch new NAME --attach` forks a branch and attaches it in one step.

Attaching converts the branch: the worker reads the root and every Lean node it
depends on on that branch, registers definition nodes in LeanGround, resolves
theorem statements, and submits nodes that carry a proof. Nodes whose proof is
`sorry` become open goals. The rules are in
[Proofs with LeanGround](../../reference/proofs/#node-conversion). If the root on
the branch resolves to a different statement than the project's root, the branch
is refused (`root_statement_differs`) and none of its proofs is submitted.
Attaching again converts the branch anew, for example after editing it.

Until the first conversion finishes, and again while a changed member list is
registered in LeanGround, the board is *initializing*: only `sync` is accepted
(`project_initializing`).

## Goals and GoalKey sharing

A **goal** is a proposition to prove. LeanGround assigns each proposition a
**GoalKey** computed from the statement and its environment; the same statement
gets the same key. Goals are deduplicated by GoalKey across the whole project, so
two nodes, two decompositions or two branches that need the same lemma share one
goal, one set of tasks and every result for it: a lemma proved on one branch is
proved on all of them, without importing anything.

Each goal has a derived status, shared by every branch:

| Status | Meaning |
| --- | --- |
| `insufficient` | No chain of accepted certificates proves it yet. |
| `derivable` | Provable by combining conditional certificates whose leaves are complete proofs. |
| `certified` | A complete certificate (no premises) exists. |

A **conditional certificate** (a *sketch*) proves a goal from premises that are
themselves goals. A cycle of sketches without a complete proof at its leaves
never makes a goal `derivable`. A goal whose negation is proved is also marked
**refuted** ([Refutation](#refutation)).

A goal's status says whether *some* combination of certificates proves it. A
branch's **route** may still be open: see [Routes](#routes-and-picks).

## Tasks, leases and budgets

Every new goal gets a `prove` task, whose holder may prove the goal or split it;
the owner can add `decompose` or `formalize` tasks (one open task per goal and
kind) with the `task` command. `mdc next` recommends and `mdc pull` claims only
`prove` tasks, so the others are taken by hand. Work on a task follows a lease:

1. `claim` a ready task with an **allocation** (budget reserved) and a lease
   time of 30–3600 seconds (default 300). Only one participant holds a task's
   lease; concurrent claims yield one winner. A claim is refused for a goal
   that is already `certified` (`already_certified`) or not reachable through
   active decompositions (`inactive_goal`, [below](#decompositions)).
2. `heartbeat` moves the lease's end to `ttl` seconds from now.
3. `finish` reports what was spent (at most the allocation,
   `allocation_exceeded`) and an outcome: `submitted`, `no_progress`,
   `execution_failed` or `released`, optionally with a [note](#notes). The task
   becomes ready again.

`heartbeat` and `finish` must come from the identity that claimed (a
[delegated token](../workspaces/#delegated-tokens) is an identity of its own) and
quote the attempt's `epoch`, so a participant whose lease lapsed or was
reassigned cannot overwrite the new holder (`stale_lease`).

A lease that expires is recorded at the next claim, sync (about every 15
seconds) or other board change: the attempt is marked `expired`, charged its
whole reservation, and the task becomes ready. A lease only avoids duplicate
work: submitting a proof needs none, and a proof submitted after the lease
expired is still accepted if it checks. Claims fail with `budget_exhausted` once
spent plus reserved plus the new allocation would exceed the project budget.
Budget units are whatever the team agrees on; MathDoc only adds up what
participants report, per identity.

Lease commands are checked against the board as it is and, except `finish` with
a note, do not change its revision, so clients send them with `If-Match: *`
([Commands](../../reference/proofs/#commands)).

## Decompositions

A **decomposition** is a conditional certificate for a goal: its source proves
the goal from new premise statements written as `sorry` theorems, and may
introduce new definitions. LeanGround's sketch component checks that the
premises imply the conclusion, not that the premises are true or easier. New
premises arrive in the project as goals with a `prove` task each; MathDoc states
each from its placeholder in the sketch, so their goal files can be pulled at
once ([New subgoals](../../reference/proofs/#new-subgoals)).

Each certificate has a state. Only `active` certificates count for scheduling,
routes and assembly; tasks can be claimed only for goals reachable from the root
through active certificates, and for the registered negations of such goals
(otherwise `inactive_goal`). The subgoals of a `proposed` decomposition can
therefore be claimed only once it is active.

| State | Meaning |
| --- | --- |
| `proposed` | A new decomposition waiting for the owner's [review](#review). Recorded, but schedules nothing. |
| `active` | Used for scheduling, routes and assembly. |
| `paused` | Kept aside; can be made active again. |
| `retired` | Not wanted; can still be made active again. |
| `refuted` | A premise was proved false ([Refutation](#refutation)). Final (`certificate_refuted`). |

The owner moves certificates between `active`, `paused` and `retired` with the
`decomposition` command; no state leads back to `proposed`. No state deletes a
fact, and no state changes what a goal's status says: a proposed or paused
decomposition can still make its goal `derivable`.

## Review

Checking a proof needs no person: assembly rechecks the whole route. What does
need one is whether a decomposition is worth working on, so on a project board:

- a complete proof is `active` at once;
- a decomposition from a document node (attaching or resubmitting a branch) is
  `active`, because the branch's editors wrote it;
- any other decomposition (an agent's or member's `submit`, an `import`, or one
  found by sync) is `proposed`, unless the project waives review (below);
- a decomposition one of whose premises is the goal itself is `retired` on
  arrival, since it can never help.

The owner reviews a proposed decomposition with the `decomposition` command,
setting it `active` (or `paused` or `retired`) with an optional comment of up to
2000 characters. From the command line, `mdc review` lists what awaits review
and `mdc review CERT --accept|--pause|--retire [-m COMMENT]` decides
([Goals, review and routes](../../reference/workspace-commands/#goals-review-and-routes));
in the browser, **Certificates** lists them under **Pending review** with each
premise's statement, the description given by the submitter, who submitted it
and which branches' routes can need its goal, with **Activate** and **Retire**.
The board keeps the last review on the certificate (`review`: who, when, state,
comment). The owner can switch a project to `policy` `review: none` (**Waive
review** in the Proof overview), so that new decompositions are active at once;
existing ones keep their state. Certificates recorded before reviews existed stay
active.

## Notes

LeanGround keeps only successful certificates, so what did not work is kept on
the goal instead: any member working on it can add a **note** (what was tried,
where it got stuck), with the `note` command, with `finish`, with `mdc note` or
`mdc done --note`, or in the goal's **Notes** tab on the browser's collaboration
page. Each goal keeps its latest 50 notes of at most 500 characters, with who
wrote them and when. `mdc pull` writes the latest 20 into the goal file; `mdc
goal` and the Notes tab show them all. A note's author changes it with
`edit_note` and removes it with `delete_note`; a person may do both for the notes
of their delegated tokens, and the owner may remove any note. MathDoc itself adds
notes, as `mathdoc`, when it refutes a goal.

## Refutation

A decomposition can be checked and still be useless: its premises imply the goal,
but one of them is false. MathDoc lets anyone prove that. `mdc refute GOAL` (or
the `negate` command) registers the negation `¬ (GOAL)` as a goal of its own,
named `not_<name>`: the worker resolves the negated statement in the project's
environment, so the negation has a GoalKey like any goal and any proof of the
same statement counts. GOAL needs a resolved statement (`goal_unresolved`).
The negation's task can be claimed while GOAL is reachable through active
certificates and not refuted; `mdc refute` registers the negation if needed and
pulls its goal file.

When a complete proof of the negation arrives, or is already on the board when
the negation is registered, GOAL is **refuted**:

- every certificate with GOAL as a premise becomes `refuted` and leaves every
  route; it cannot be made active again;
- the tasks of GOAL, and of goals that only those certificates reached, are
  cancelled, ending their leases (charged their reservation);
- GOAL and the goal of each refuted decomposition get a note saying why, so its
  submitter finds it on the task board and in the next goal file.

Only a complete proof refutes: a sketch of the negation refutes nothing, even
when its premises are proved and the negation is `derivable`.

## What to do next

`mdc next` (`GET …/next`, and the order of the task board) recommends free goals
worth working on: goals with a ready `prove` task on the frontier of the attached
branches' routes, that is, needed by a route, not yet provable from active
certificates, not `certified` or refuted, and with no active decomposition of
their own. Each route that needs a goal adds 1 ÷ the number of unproved goals the
route still needs, so routes close to closing count more; the sum is divided by
1 + the goal's failed attempts (no progress, execution failure or an expired
lease). A registered negation scores as its goal. Ties go to fewer failures,
then to the GoalKey. `--branch B` counts only B's route; `mdc next --claim` pulls
the first recommendation.

## Routes and picks

Each branch **picks** decompositions: a map from goals to the certificate its
route uses there. Converting or resubmitting a node records the node's own
certificate as the branch's pick for its goal, so a branch's document is its
route; converting the branch again sets those picks back. The owner, and any
member who may write the branch (its editors), can change a pick with the `pick`
command: `mdc pick GOAL CERT --branch B` (`auto` instead of CERT clears it), or
in the browser **Use on this branch** and **Unpick** in Certificates. A
branch's route is part of its content, so it follows the branch's write access.
`mdc branch new NAME --pick GOAL=CERT` opens a new route in one step: it forks the
branch, attaches it to the project, waits for its conversion and then sets the
picks.

A branch's route uses the picked certificate at every picked goal, and chooses
automatically elsewhere (active certificates with the fewest premises). A picked
certificate that is not active leaves its goal open on that route. The **Proof
overview** shows, per branch, whether its route **closes**, that is, reaches
complete proofs. Two branches can therefore share every proved lemma and still
differ in how they split the root.

## Assembly

When a branch's route closes, the owner **assembles** for that branch
(**Assemble a route** in the browser's **Assemble & write back** tab): MathDoc selects the route
from the branch's picks and asks LeanGround to combine the certificates into one
source without premises, checked from scratch by `lean-worker`. One assembly runs
at a time per project (`assembly_in_progress`); a route that does not close is
refused at once (`no_active_route`). A failed assembly marked retryable can be
retried with the same plan; a new route is a new assembly.

## Writeback

Accepting a certified assembly writes its sources back into **the assembly's
branch** as one batch, in a single TerminusDB commit; other branches are not
touched.

- Every existing node on the route must still have the Lean text it had when it
  was converted; otherwise the whole batch is `aborted` with
  `document_conflict` and nothing is written. Restore or resubmit, then accept
  again.
- Nodes whose own text was certified get only metadata. Nodes proved elsewhere,
  by a participant or on another branch, get the certified declarations, keeping
  their `import` and context lines; the conclusion's name must stay the same.
- Subgoals from decompositions without a node, and definitions without a node,
  become new nodes with edges to what they use.
- Lean block metadata records `certification_id`, `certified_lean_sha256` and the
  batch ID; the root records the final assembly certificate.

Replaying a batch never creates nodes twice. See
[Proofs with LeanGround](../../reference/proofs/#assembly-and-writeback) for details.

## Staleness

A branch becomes **stale** when a sync finds that the Lean text of its root, a
direct premise or a definition it uses has changed since the branch was
converted, or that the branch was deleted. Its results stay in history, but
`accept` into that branch is refused (`proof_request_stale`); submit the root
again to convert the branch anew. Other branches are unaffected. Independently,
a node whose text or definitions changed shows **Changed since submission**
instead of Certified. A writeback's own changes are the new baseline and do not
make the branch stale.

A node on a branch that is not attached, or that its branch never converted,
shows the status of the same node on an attached branch when its Lean text and
the definitions it uses are identical there.

## Read-only boards

Two kinds of boards stay readable but accept no commands and are no longer
synced:

- **Superseded projects.** Changing the database's proof environment changes
  every GoalKey, so the old board is marked superseded (`project_superseded`).
  The next submission creates a new project board in the new environment;
  setting an earlier environment again revives the board made in it.
- **Per-branch proof requests** from MathDoc before project boards
  (`proof_request_read_only`). Their node statuses are still shown until a
  project board converts the same nodes.

## Permissions

Collaboration never widens document access:

- The project board is visible to its members who can read the database's
  `main` branch; branches a member cannot read, with their assemblies, are left
  out of what they see. Goals and certificates are shared by the whole project,
  including those that came from such a branch: keep branches that some members
  must not read in a separate database.
- Creating a project needs `owner` on `main` (and `editor` on the branch);
  attaching a branch needs `editor` or better on that branch; a member's `pick`
  needs `editor` on its branch; accepting writes needs `editor` on the assembly's
  branch.
- Every member must be able to read `main`; adding one who cannot fails with
  `member_lacks_document_access` (`unknown_member` for a name not in
  `MDC_ACTORS`).
- The owner alone changes members, adds tasks, reviews and manages
  decompositions, sets the review policy, assembles, retries and accepts, and
  picks on any branch. Members claim tasks, submit, resolve, import, resubmit
  nodes, add notes, register negations, attach and pick on branches they can
  edit, keep the project notes and sync. Non-members get `404 not_found`; a
  member sending an owner command gets `403 owner_required`.
- A delegated token acts with its person's membership, ownership and roles,
  within its scopes: `work` for the members' commands, `edit` for attaching and
  for an editor's pick, `maintain` for the owner's commands and for creating a
  project, `maintain` and `edit` for `accept` (`403 token_scope_required`
  otherwise).

Changing members revokes the open leases of removed members (charged their
reservation); a delegated token keeps its lease while its person stays a member.
