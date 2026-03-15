import inspect
import sys
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.ui import BrokenDependencySummary
from mathdoc.ui import ChainView
from mathdoc.ui import CycleView
from mathdoc.ui import DepAddView
from mathdoc.ui import DepRmView
from mathdoc.ui import EvalBlockView
from mathdoc.ui import EvalReportView
from mathdoc.ui import GraphCheckView
from mathdoc.ui import IssueView
from mathdoc.ui import MissingReferrerView
from mathdoc.ui import NodeRef
from mathdoc.ui import TerminalUI


def _public_terminal_methods() -> list[str]:
    return sorted(
        name
        for name, value in inspect.getmembers(TerminalUI, predicate=inspect.isfunction)
        if not name.startswith("_")
    )


def _section(title: str, when: str) -> None:
    print()
    print("=" * 80)
    print(title)
    print(f"When you see this: {when}")
    print("-" * 80)


def _sample_refs() -> dict[str, NodeRef]:
    return {
        "source": NodeRef(
            fnode="11111111-1111-1111-1111-111111111111",
            title="Source Card",
            rel_path="notes/source-card.mdoc",
        ),
        "dep_1": NodeRef(
            fnode="22222222-2222-2222-2222-222222222222",
            title="Dependency One",
            rel_path="notes/dependency-one.mdoc",
            depth=1,
        ),
        "dep_2": NodeRef(
            fnode="33333333-3333-3333-3333-333333333333",
            title="Dependency Two With A Surprisingly Long Title",
            rel_path="notes/dependency-two.mdoc",
            depth=2,
        ),
        "missing": NodeRef(
            fnode="missing-target-001",
            title="<missing>",
            rel_path="<unknown>",
            depth=1,
            broken=True,
        ),
        "invalid": NodeRef(
            fnode="44444444-4444-4444-4444-444444444444",
            title="<invalid>",
            rel_path="notes/broken-card.mdoc",
            depth=2,
            broken=True,
        ),
        "cycle_a": NodeRef(
            fnode="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            title="Cycle A",
            rel_path="cycles/a.mdoc",
        ),
        "cycle_b": NodeRef(
            fnode="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            title="Cycle B",
            rel_path="cycles/b.mdoc",
        ),
        "cycle_c": NodeRef(
            fnode="cccccccc-cccc-cccc-cccc-cccccccccccc",
            title="Cycle C",
            rel_path="cycles/c.mdoc",
        ),
    }


def preview_terminal_ui() -> int:
    ui = TerminalUI()
    refs = _sample_refs()
    covered: set[str] = set()

    _section(
        "write",
        "Low-level plain terminal output. Other helpers eventually flow through this.",
    )
    covered.add("write")
    ui.write("Plain text written directly by TerminalUI.write()")

    _section(
        "write_lines",
        "Low-level multi-line output. Most render_* helpers are printed through this.",
    )
    covered.add("write_lines")
    ui.write_lines(
        [
            "First line from TerminalUI.write_lines()",
            "Second line from TerminalUI.write_lines()",
        ]
    )

    _section(
        "error",
        "Any CLI command that needs to stop immediately with an error message.",
    )
    covered.add("error")
    ui.error("sample fatal error")

    _section(
        "warning",
        "General warning output when a command can continue but wants to alert the user.",
    )
    covered.add("warning")
    ui.warning("sample warning")

    _section(
        "hint",
        "Follow-up guidance, usually after an error or degraded state.",
    )
    covered.add("hint")
    ui.hint("sample hint")

    _section(
        "info",
        "Long-running progress updates, such as Lean workspace setup or compilation steps.",
    )
    covered.add("info")
    ui.info("sample progress update")

    _section(
        "format_node_ref",
        "Low-level single-node formatter used by search results, dependency chains, graph issues, and mutation summaries.",
    )
    covered.add("format_node_ref")
    ui.write(ui.format_node_ref(refs["dep_1"]))
    ui.write(ui.format_node_ref(refs["dep_2"], include_depth=True))
    ui.write(ui.format_node_ref(refs["invalid"], include_depth=True))

    _section(
        "format_issue",
        "Low-level graph-check issue row, usually inside missing/invalid sections.",
    )
    covered.add("format_issue")
    ui.write(
        ui.format_issue(
            IssueView(
                ref=refs["missing"],
                error="no mdoc matched reference: missing-target-001",
            )
        )
    )

    _section(
        "render_anchor_error_lines",
        "Commands like `mdc eval`, `mdc dep show`, or `mdc dep rm` when a source/target node is known but a follow-up operation fails.",
    )
    covered.add("render_anchor_error_lines")
    ui.write_lines(
        ui.render_anchor_error_lines(
            label="source",
            item=refs["source"],
            message="failed to inspect dependencies: sample failure",
        )
    )

    _section(
        "render_anchor_message_lines",
        "Commands that want to show a source/target row plus a neutral follow-up message, such as `No dependencies to remove`.",
    )
    covered.add("render_anchor_message_lines")
    ui.write_lines(
        ui.render_anchor_message_lines(
            label="source",
            item=refs["source"],
            message="No dependencies to remove",
        )
    )

    _section(
        "render_chain_lines",
        "Shared chain output for `mdc dep show`, `mdc dep refs`, and the dependency chain printed at the start of `mdc eval`.",
    )
    covered.add("render_chain_lines")
    ui.write_lines(
        ui.render_chain_lines(
            ChainView(
                anchor_label="source",
                anchor=refs["source"],
                count_label="dependencies",
                items=(refs["dep_1"], refs["dep_2"], refs["invalid"]),
            )
        )
    )

    _section(
        "render_broken_dependency_warning_lines",
        "Shown after dependency-chain style output when missing/invalid refs were detected. Eval and non-eval wording differ.",
    )
    covered.add("render_broken_dependency_warning_lines")
    summary = BrokenDependencySummary(missing=1, invalid=1)
    ui.write_lines(
        ui.render_broken_dependency_warning_lines(summary=summary, for_eval=False)
    )
    ui.write_lines(
        ui.render_broken_dependency_warning_lines(summary=summary, for_eval=True)
    )

    _section(
        "render_missing_referrer_lines",
        "Shown after dependency output when a missing target needs explicit referrer context.",
    )
    covered.add("render_missing_referrer_lines")
    ui.write_lines(
        ui.render_missing_referrer_lines(
            (
                MissingReferrerView(
                    target=refs["missing"],
                    referrers=(refs["dep_1"], refs["source"]),
                ),
            )
        )
    )

    _section(
        "render_index_error_lines",
        "Used when index bootstrap/search/sync fails, for example if `.mdc/index.db` is corrupted.",
    )
    covered.add("render_index_error_lines")
    ui.write_lines(
        ui.render_index_error_lines(
            action="prepare search index",
            exc=ValueError("sample sqlite/index problem"),
        )
    )

    _section(
        "warn_index_failure",
        "Used when the main command succeeded but the follow-up incremental index refresh failed.",
    )
    covered.add("warn_index_failure")
    ui.warn_index_failure(
        "mdoc was edited", ValueError("sample incremental update failure")
    )

    _section(
        "render_search_results_lines",
        "Standard output for `mdc search <query>` when matches were found.",
    )
    covered.add("render_search_results_lines")
    ui.write_lines(
        ui.render_search_results_lines(
            query="dep",
            matches=[refs["dep_1"], refs["dep_2"]],
        )
    )

    _section(
        "render_created_lines",
        "Shown after `mdc new` creates a new node.",
    )
    covered.add("render_created_lines")
    ui.write_lines(
        ui.render_created_lines(
            path="/tmp/demo/source-card.mdoc",
            root_item=refs["source"],
        )
    )

    _section(
        "render_dep_add_lines",
        "Shown after `mdc dep add` writes one or more direct dependencies.",
    )
    covered.add("render_dep_add_lines")
    ui.write_lines(
        ui.render_dep_add_lines(
            DepAddView(
                source=refs["source"],
                added=(
                    NodeRef(
                        fnode=refs["dep_1"].fnode,
                        title=refs["dep_1"].title,
                        rel_path=refs["dep_1"].rel_path,
                    ),
                    NodeRef(
                        fnode=refs["dep_2"].fnode,
                        title=refs["dep_2"].title,
                        rel_path=refs["dep_2"].rel_path,
                    ),
                ),
                skipped_existing=1,
                skipped_self=1,
            )
        )
    )

    _section(
        "render_dep_rm_lines",
        "Shown after `mdc dep rm` removes one or more direct dependencies.",
    )
    covered.add("render_dep_rm_lines")
    ui.write_lines(
        ui.render_dep_rm_lines(
            DepRmView(
                source=refs["source"],
                removed=(
                    NodeRef(
                        fnode=refs["dep_1"].fnode,
                        title=refs["dep_1"].title,
                        rel_path=refs["dep_1"].rel_path,
                    ),
                    refs["missing"],
                ),
            )
        )
    )

    _section(
        "render_cycle_lines",
        "Used by `mdc dep show`, `mdc eval`, and `mdc graph check` when a dependency cycle is detected.",
    )
    covered.add("render_cycle_lines")
    ui.write_lines(
        ui.render_cycle_lines(
            CycleView(nodes=(refs["cycle_a"], refs["cycle_b"], refs["cycle_c"]))
        )
    )

    _section(
        "render_graph_check_lines",
        "Full report output for `mdc graph check`.",
    )
    covered.add("render_graph_check_lines")
    ui.write_lines(
        ui.render_graph_check_lines(
            GraphCheckView(
                nodes=7,
                edges=9,
                missing=(
                    IssueView(
                        ref=refs["missing"],
                        error="no mdoc matched reference: missing-target-001",
                    ),
                ),
                invalid=(
                    IssueView(
                        ref=refs["invalid"],
                        error="duplicate @title found at line 12",
                    ),
                ),
                cycles=(
                    CycleView(
                        nodes=(refs["cycle_a"], refs["cycle_b"], refs["cycle_c"])
                    ),
                ),
            )
        )
    )

    _section(
        "render_eval_block_start_lines",
        "Streaming block header shown before each block begins execution.",
    )
    covered.add("render_eval_block_start_lines")
    ui.write_lines(ui.render_eval_block_start_lines(index=1, total=2, srctype="natl"))

    _section(
        "render_eval_block_lines",
        "Non-streaming single-block rendering that includes both block header and completion status.",
    )
    covered.add("render_eval_block_lines")
    ui.write_lines(
        ui.render_eval_block_lines(
            EvalBlockView(
                index=1,
                total=2,
                srctype="natl",
                ok=True,
                rtcode=0,
                stdout="merged natural-language body",
                stderr="",
            )
        )
    )

    _section(
        "render_eval_block_finish_lines",
        "Streaming block footer shown when a block finishes, after any block output.",
    )
    covered.add("render_eval_block_finish_lines")
    ui.write_lines(
        ui.render_eval_block_finish_lines(
            EvalBlockView(
                index=2,
                total=2,
                srctype="py",
                ok=False,
                rtcode=1,
                stdout="",
                stderr="RuntimeError: sample failure",
            )
        )
    )

    _section(
        "render_eval_results_lines",
        "Block execution summary shown after `mdc eval` actually compiles/runs blocks.",
    )
    covered.add("render_eval_results_lines")
    ui.write_lines(
        ui.render_eval_results_lines(
            EvalReportView(
                blocks=(
                    EvalBlockView(
                        index=1,
                        total=2,
                        srctype="natl",
                        ok=True,
                        rtcode=0,
                        stdout="merged natural-language body",
                        stderr="",
                    ),
                    EvalBlockView(
                        index=2,
                        total=2,
                        srctype="py",
                        ok=False,
                        rtcode=1,
                        stdout="",
                        stderr="RuntimeError: sample failure",
                    ),
                ),
                failed=1,
            )
        )
    )

    _section(
        "render_synced_lines",
        "Shown after `mdc sync` completes successfully.",
    )
    covered.add("render_synced_lines")
    ui.write_lines(ui.render_synced_lines(42))

    _section(
        "render_edited_lines",
        "Shown after `mdc edit` returns from $EDITOR and updates the index successfully.",
    )
    covered.add("render_edited_lines")
    ui.write_lines(ui.render_edited_lines("notes/source-card.mdoc"))

    declared = set(_public_terminal_methods())
    missing = sorted(declared - covered)
    extra = sorted(covered - declared)

    print()
    print("=" * 80)
    print("Coverage check")
    print("-" * 80)
    if missing or extra:
        if missing:
            print(f"Missing preview coverage: {', '.join(missing)}")
        if extra:
            print(f"Preview referenced unknown methods: {', '.join(extra)}")
        return 1

    print(f"All TerminalUI public methods covered: {len(declared)}")
    return 0


def main() -> int:
    print("TerminalUI preview")
    print("Tip: run this in a real TTY to see colors and glyphs exactly as users will.")
    return preview_terminal_ui()


if __name__ == "__main__":
    raise SystemExit(main())
