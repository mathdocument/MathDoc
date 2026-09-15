---
title: LaTeX previews and references
---

LaTeX blocks are document bodies. Shared macros and bibliography belong to the
branch's LaTeX project, configured with `mdc project latex set --preamble macros.tex
--bib references.bib -p myproject/main`. The macro file may also be a `.cls` file.
Only filenames and content are stored; the service does not watch the source files.

## Editing

Open **Project settings → LaTeX**, select a `.cls` or `.tex` file and a `.bib`
file, then save. The CLI command above performs the same operation. The two
file contents are versioned in the branch; later changes to the local files
require uploading them again.

Each block has an **Edit / Preview** toggle. Preview preparation is debounced
while typing and uses the unsaved draft; only **Save** writes the node. The
read-only imported-dependency list is derived from `dep` and never inserted
into the block source. Reference, citation, macro and environment completions
appear as you type, or with **Ctrl+Space**. References are scoped to the selected
node; bibliography candidates are shared by the branch. Completion inserts a
qualified external key while showing the target's readable title.

Click a reference to open its node and scroll to the label in Preview. Opening
the link in a new tab also targets that label. Citation links scroll to the
node's bibliography. Dependency and project changes are checked every five
seconds while the editor is visible; stale draft responses are discarded.
No TeX language server is started.

## Shared macros

Use ordinary LaTeX definitions, for example:

```tex
\newcommand{\cA}{\mathcal{A}}
\newcommand{\Hom}{\operatorname{Hom}}
\newenvironment{items}{\begin{itemize}}{\end{itemize}}
\newtheorem{thm}{Theorem}
```

plasTeX expands macros and parses document structure. The web preview uses KaTeX
for math. This is HTML generation, not XeLaTeX compilation: page layout, font
configuration, arbitrary packages, filesystem input and external commands are
not supported. Class declarations may accompany ordinary macros; a class that
requires engine-specific or complex layout code must be reduced to its content
macros. There are no special rewrites for particular users' classes or aliases.
Unsupported content reports a diagnostic rather than being silently discarded.

## References

`dep` is the only source of external imports. A preview sees the current node's
labels and labels exported by its **direct** dependencies. Unused dependencies
are allowed. Source-level `\externaldocument` and filesystem `\input` are rejected.

A local label is written normally, such as `\label{thm:main}`. External labels have
stable qualified keys `NODE_UUID::thm:main`. An unqualified external label also
works if it identifies exactly one permitted target; local labels take precedence.
Ambiguous labels require a qualified key. Removing a dependency immediately
removes access to its labels; transitive dependencies are not implicitly imported.

`\nameref` displays the theorem's optional title or the section title. For an
unnamed target, it uses the node title and local label. `\ref`, `\cref`, `\Cref`
and `\eqref` retain their usual number/type presentation with local numbering.
References carry the target UUID and label so the web client can navigate to the
correct node without reading an `.aux` file or building a PDF.

`\cite{key}` uses the shared BibTeX database. Pybtex produces alphabetic citation
labels and formatted bibliography entries; only the entries cited by this node
appear in its preview. Optional citation notes, such as `\cite[Theorem 2]{key}`,
are retained. The HTML citation style is independent of a print-only `.bst` file.

## Runtime and caching

Python 3.9+ with venv support is required. First use installs the pinned plasTeX and
Pybtex dependencies into a shared virtual environment under the MDC cache root,
then reuses it across branches. Initial installation requires network access.
For preinstalled/offline deployments, install `src/latex/requirements.txt` into a
virtual environment and set `MDC_LATEX_PYTHON` to its Python executable.

Each branch lazily starts one rendering worker. Parsed results are content-keyed
and bounded to 128 entries / 32 MiB; bibliography results are cached separately.
Each document gets its own macro context. Draft requests never update persisted
node labels or another client's document. Unchanged context queries return only
version information, and rendering checks again for dependency changes before
returning a result. No full-graph compilation or workspace synchronization occurs.
Rendering has a 10-second timeout and bounded request/output sizes. Stopping a
branch also stops its renderer.
