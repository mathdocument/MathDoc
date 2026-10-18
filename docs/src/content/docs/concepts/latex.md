---
title: LaTeX previews and references
---

LaTeX blocks are document bodies. Shared macros and bibliography belong to the
branch's LaTeX project, configured with `mdc project latex set --preamble macros.tex
--bib references.bib -p myproject/main` (both files are required). The macro
file may also be a `.cls` file. Only filenames and content are stored; the
service does not watch the source files.

## Editing

Open **Project settings → LaTeX**, select a `.cls` or `.tex` file, a `.bib`
file or both, then **Save LaTeX project**. The CLI command above performs the
same operation. The two file contents are versioned in the branch and copied to
branches forked from it; later changes to the local files require uploading them
again.

Saving stores the uploaded text without parsing or formatting it. Only filenames
(`.tex`/`.cls` and `.bib`, without paths), sizes (2 MiB macro file, 16 MiB
bibliography) and the branch revision are checked; incomplete LaTeX or BibTeX
can be saved. Parsing errors are reported when loading completions or previews.

Each LaTeX block has an **Edit / Preview** toggle. The selected mode is shared by
LaTeX blocks in the current page: switching nodes keeps it, while a new page
starts in Edit unless opened at a specific reference. Reading-mode navigation
prepares the destination preview before switching nodes. Initial rendering starts
immediately; only typing is debounced (250 ms). Previews use the unsaved draft;
only **Save** writes the node. The source editor is created when entering Edit,
so reading another node does not initialize a hidden editor. Once opened, it
stays available for Edit / Preview switches on that node. The
imported-dependency list stays visible in both Edit and Preview, is derived from
`dep`, and never enters the block source. Rendering diagnostics, such as
unknown references or unsupported commands, are listed below the block.

Reference, citation, macro and environment completions appear as you type, or
with **Ctrl+Space**; Enter or Tab accepts one. References are offered inside
`\ref`, `\cref`, `\Cref`, `\nameref` and `\eqref`, citations inside `\cite`.
References are scoped to the selected node; bibliography candidates are shared
by the branch. Reference completion shows each target's title (or label) and
its node's title, and inserts `NODE_UUID::label` for dependencies, or just
`label` within the current node. Matching and relevance scores use Monaco's
native fuzzy matcher. Citation searches include the key, title, authors and
year. Each list shows the best 50 matches from the entire catalog in a natively
scrolling list; the display limit does not restrict which entries can be found.

Click a reference to open its node and scroll to the label in Preview. Opening
the link in a new tab also targets that label. The toolbar's **Back** /
**Forward** buttons follow browser history, keep the selected Edit / Preview
mode and confirm before discarding unsaved edits. Citation links scroll to the
node's bibliography. Dependency and project changes are checked every five
seconds while the editor is visible; slow context requests are allowed to finish
and stale draft responses are discarded. Completion catalogs must match the
requested project version before they can be cached.
No TeX language server is started.

Source editors of every block type start with at least 10rem (about 160px) of
writing space and grow with their content up to the node pane's visible height;
in short windows the visible-height cap takes priority.
Long editors and LaTeX previews scroll inside their block, leaving its toolbar
in place. At a block's boundary, scrolling continues in the surrounding node
pane for the rest of the gesture, without entering another editor. The pane
keeps the browser's native edge feedback; individual editors do not rubber-band.

## Shared macros

Use ordinary LaTeX definitions, for example:

```tex
\newcommand{\cA}{\mathcal{A}}
\newcommand{\Hom}{\operatorname{Hom}}
\newenvironment{items}{\begin{itemize}}{\end{itemize}}
\newenvironment{prf}{\begin{proof}}{\end{proof}}
\newtheorem{thm}{Theorem}
```

plasTeX expands macros and parses document structure. The web preview uses MathJax 4
for math. This is HTML generation, not XeLaTeX compilation: page layout, font
configuration, arbitrary packages, filesystem input and external commands are
not executed. Upload the original class or preamble: MathDoc imports ordinary macro,
environment, math-operator and theorem declarations while skipping global setup
calls and their arguments. Unsupported package declarations such as `etoolbox`,
`geometry` and `fontspec` do not prevent independent macros from loading. The
supported built-in plasTeX packages still supply their content commands:
`amsmath`, `amssymb`, `amsfonts`, `amsthm`, `mathtools`, `mathrsfs`, `xcolor`
(also as `color`), `hyperref`, `cleveref` and `tikz-cd`. In a node body,
`\usepackage` of any other package is an error.

Proof environments retain optional captions, including through aliases:
`\begin{prf}[Sketch of the proof] ... \end{prf}`. Preview text uses the bundled
Latin Modern Roman font at 16px, independently of the class's print fonts. Its
complete OpenType regular, italic, bold and bold-italic faces retain accented
letters such as `ö`, `é` and `ü` in the same typeface, including bibliography
titles. These TeX fonts are served locally under the GUST Font License; clients
do not need a TeX installation. Ordinary formulas use MathJax's official
TeX font package (Computer Modern with AMS blackboard bold), pinned with
MathJax (4.1.3) and bundled locally. This follows conventional LaTeX/`amssymb`, rather
than Latin Modern Math's different `\mathbb` alphabet; class-specific font
selections are not applied to HTML. No CDN requests are needed. Math is rendered
asynchronously, and closing or changing a preview releases its MathJax items
without affecting other previews.
TikZ diagrams use the separate TeX-to-SVG runtime described below.
Inline formulas use the surrounding text's font size; display formulas retain
their separate scale. Normal text is not artificially emboldened.

Numbered theorem environments show their number in the heading. Each node starts
at 1, retaining the class's independent or shared theorem counters. Section
prefixes and section-based resets from `\newtheorem` are ignored. Starred
environments stay unnumbered. Theorem and proof headings run into the first
paragraph; later paragraphs, lists and display equations retain their own layout.

Macro bodies are stored without executing them. For example, an unused title-page
macro containing `\ifstrempty` does not require `etoolbox` support. Calling that
macro in a node still reports an unsupported command; skipping a package does not
implement its macros. Macros generated by executing setup loops or hooks are not
imported. Ordinary definitions should be declared directly.

The supported subset includes ordinary `\def`/`\gdef`, LaTeX command and
environment definitions (including optional arguments), and math operators.
It does not implement TeX's eager expansion in `\edef`/`\xdef`: unused definitions
are retained, but using one reports an error instead of silently treating it as
`\def`. Arbitrary TeX programs are not guaranteed to match a TeX engine.

Supported TeX conditionals and `\newif` flags select their active declarations;
class checks use the HTML preview's `article` context, ignoring print class options.
Unknown primitive conditionals import neither branch. Grouped setup arguments
and unused macro bodies never contribute definitions accidentally. `\endinput`
ends the shared input without consuming the node body. There are no special
rewrites for particular users' classes or aliases. Unsupported node content still
reports a diagnostic rather than being silently discarded.

## Tables, colors and diagrams

`tabular` and `tabular*` render as HTML tables, including column alignment,
rules and `\multicolumn`. Empty outer `@{}` insertions remove outer cell padding
without shifting column alignment or vertical rules. Booktabs `\toprule` and
`\bottomrule` use their standard `.08em` width, `\midrule` uses `.05em`, and
`\cmidrule` uses `.03em`; optional rule widths are retained. Table rules follow
the surrounding text color in both themes. Column widths follow HTML layout; the parser does not
preserve the fixed width of `p{...}` columns. Their contents retain math and
graph references. `\color`, `\textcolor`, `\colorbox` and `\fcolorbox` use
xcolor's named colors and color expressions; shared `\definecolor` and
`\colorlet` declarations apply to text and math.

`tikzcd` renders as SVG with the bundled TikZJax TeX WebAssembly runtime, in a
browser worker. A `tikzcd` inside a formula is rendered together with that
formula. Standard arrows, labels, TikZ options and shared macro definitions
are interpreted by TeX. The runtime and fonts load on the first diagram, from
the MathDoc server; no CDN, host TeX installation or additional Docker service is
required. Rendering is serialized per browser page, with a 30-second limit, an
8 MiB SVG limit and a cache of the last 32 diagrams keyed by diagram source and
preamble.

A diagram receives only the shared declarations it uses (with the declarations
those use), color definitions and `\usetikzlibrary`/`\tikzset` settings; print
layout setup is skipped. TeX reads only bundled runtime files, and generated SVG
is sanitized before display. Diagram labels are
local TeX content; graph `\ref` and bibliography links belong in the surrounding
HTML body. The diagram keeps a white drawing surface so explicit colors retain
their TeX meaning in either UI theme.

```tex
\begin{tabular}{|l|c|}
\hline Object & Value \\
\hline $A$ & \textcolor{blue}{1} \\
\hline
\end{tabular}

\begin{tikzcd}
A \arrow[r,"f"] \arrow[d,"g"'] & B \arrow[d,"h"] \\
C \arrow[r,"k"'] & D
\end{tikzcd}
```

## References

`dep` is the only source of external imports. A preview sees the current node's
labels and labels exported by its **direct** dependencies. Unused dependencies
are allowed. Node source may not use `\input`, `\include`, `\externaldocument`,
`\includegraphics`, `\bibliography`, `\bibliographystyle`, `\openin`,
`\openout`, `\read`, `\write`, `\immediate` or `\special`; they are rejected.

A label is written normally, such as `\label{thm:main}`. In the web editor,
type `thm:main` inside `\cref{…}`, `\nameref{…}` or another reference command
and accept a suggestion with Enter: external references insert
`NODE_UUID::thm:main` using the target's full UUID; references within the
current node insert only `thm:main`. Completion also searches theorem and node
titles. Agents using `mdc edit --type latex` should write the qualified form
directly for external references.

Bare labels remain supported when they identify exactly one target among the
current node and its direct dependencies; collisions report an ambiguity
instead of silently preferring the local label. Qualified references let
different dependencies reuse the same label without ambiguity.
Removing a dependency immediately removes access to its labels; transitive
dependencies are not implicitly imported.

Use `\cref` for numbered references: a local reference reads `Theorem 1`, and a
cross-node reference reads `12345678::Theorem 1`. The prefix is the target
node's first eight UUID characters; navigation still uses its full UUID.
External `\ref`, `\Cref` and `\eqref` also show the target type and number.
Within a node, `\ref` displays the number and `\eqref` puts it in parentheses.

For numbered external targets, `\nameref` uses the same type and number as
`\cref`. Within the current node, it retains the theorem's optional title or the
section title. An unnumbered external target reads `12345678::Type (Title)`,
where a target without a title of its own uses `Node title — label`; `\nameref`
to a titled one reads just `12345678::Title`.
References carry the target UUID and label so the web client can navigate to the
correct node without reading an `.aux` file or building a PDF.

`\cite{key}` uses the shared BibTeX database. Pybtex's BibTeX interpreter runs the
bundled, unmodified AMS `amsalpha.bst` (version 2.0, under LPPL 1.3c or later).
It supplies alphabetic labels, ordering and entry formatting; plasTeX renders the
result as HTML with italic titles and bold journal volumes. No BibTeX executable
or TeX installation is required. Optional citation notes such as
`\cite[Theorem 2]{key}` are retained.

Only cited entries and their BibTeX `crossref` parents are processed. Label
suffixes such as `a` and `b`, and repeated-author dashes, follow this node's cited
set. Missing fields such as `editor`, `author` or `year` remain nonblocking BibTeX
warnings; no missing metadata is invented. Completion reads metadata without
formatting the whole library. Preview caches each cited set, bounded to 128 sets
and 8 MiB per cached bibliography. Uploaded BibTeX `@preamble` code is not executed;
put ordinary content macros in the shared `.cls` or `.tex` file.

## Runtime and caching

The API process runs the Python renderer in `renderer/` (`MDC_RENDERER_DIR` in the
image). Python 3.9+ with venv support is required. Unless `MDC_LATEX_PYTHON` is
set, first use installs the pinned plasTeX 3.1 and Pybtex 0.26.1 into a shared
virtual environment under `MDC_CACHE_DIR/runtime/` (an absolute path; default
`$XDG_CACHE_HOME/mathdoc/runtime/`, or `~/.cache/mathdoc/runtime/`) and reuses it
for all branches; this needs `python3` and network access once. For offline
hosts, install `renderer/requirements.txt` into a virtual environment and set
`MDC_LATEX_PYTHON` to its Python executable. The Docker image ships such an
environment in `/opt/latex`.

Each branch lazily starts one rendering worker, which handles one request at a
time; when 8 requests of a branch are already waiting or running, more are
refused as busy. Parsed results are content-keyed
and bounded to 128 entries / 32 MiB; bibliography results are cached separately.
The shared preamble and bibliography stay resident in that worker. Their content
hash is retained with the branch snapshot, and the full configuration is sent
only when its content changes or the worker restarts. Switching nodes and saving
ordinary node edits do not retransmit the bibliography. Other branches retain
their own configuration independently.
Shared declarations are parsed once per preamble. Each document gets an isolated
copy of that compiled macro context, including counters, conditional flags and
color definitions; dependency bodies and bibliography fragments reuse the same
preparation. Draft requests never update persisted
node labels or another client's document. Unchanged context queries return only
version information, and rendering checks again for dependency changes before
returning a result. No full-graph compilation or workspace synchronization occurs.
Rendering has a 10-second timeout and bounded sizes: 2 MiB per LaTeX block,
32 MiB per renderer request or response (the block, its dependencies' blocks
and, when it changed, the configuration), 8 MiB of rendered HTML and 200,000
expanded tokens per parse. After a timeout, or when the renderer stops or exceeds
its response limit, the next request starts a new one. A branch's renderer stops
when the branch is deleted or the API process exits.
