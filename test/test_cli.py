import os
import pty
import re
import select
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

CLI_BOOTSTRAP = "from mathdoc.cli import main; raise SystemExit(main())"
ANSI_ESCAPE_RE = re.compile(r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
ANSI_ESCAPE_BYTES_RE = re.compile(br"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
SEARCH_RESULT_RE = re.compile(r"^- (?:[0-9a-f]{8}|<[^>]+>) ")
CHAIN_ITEM_RE = re.compile(r"^\[\d+\] [+-] ")
EVAL_BLOCK_RE = re.compile(r"^\[\d+\] [A-Za-z0-9_+-]+: (?:ok|failed \(\d+\))$")


def _cli_base_cmd() -> list[str]:
    return [sys.executable, "-c", CLI_BOOTSTRAP]


def _cli_env() -> dict[str, str]:
    env = os.environ.copy()
    old = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = str(
        SRC_PATH) if not old else f"{SRC_PATH}{os.pathsep}{old}"
    return env


def _run_cli(
    args: list[str], cwd: Path, env_extra: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    env = _cli_env()
    if env_extra:
        env.update(env_extra)
    return subprocess.run(
        _cli_base_cmd() + args,
        cwd=str(cwd),
        env=env,
        text=True,
        capture_output=True,
    )


def _run_cli_tty_script(
    args: list[str],
    cwd: Path,
    script: list[tuple[bytes, bytes]],
) -> tuple[int, str]:
    master_fd, slave_fd = pty.openpty()
    try:
        proc = subprocess.Popen(
            _cli_base_cmd() + args,
            cwd=str(cwd),
            env=_cli_env(),
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            close_fds=True,
        )
    except Exception:
        os.close(slave_fd)
        os.close(master_fd)
        raise
    os.close(slave_fd)

    output = bytearray()
    step_index = 0
    deadline = time.time() + 15.0
    try:
        while time.time() < deadline:
            ready, _, _ = select.select([master_fd], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(master_fd, 4096)
                except OSError:
                    break
                if not chunk:
                    break
                output.extend(chunk)
                while step_index < len(script):
                    trigger, response = script[step_index]
                    if trigger not in ANSI_ESCAPE_BYTES_RE.sub(b"", bytes(output)):
                        break
                    os.write(master_fd, response)
                    step_index += 1

            if proc.poll() is not None:
                while True:
                    ready2, _, _ = select.select([master_fd], [], [], 0)
                    if not ready2:
                        break
                    try:
                        tail = os.read(master_fd, 4096)
                    except OSError:
                        break
                    if not tail:
                        break
                    output.extend(tail)
                break

        if proc.poll() is None and step_index < len(script):
            pending_trigger = script[step_index][0].decode("utf-8", errors="replace")
            cleaned_output = _clean_cli_output(
                output.decode("utf-8", errors="replace")
            )
            proc.kill()
            proc.wait()
            raise AssertionError(
                f"TTY script timed out waiting for trigger {pending_trigger!r}.\n"
                f"Output so far:\n{cleaned_output}"
            )
    finally:
        try:
            os.close(master_fd)
        except OSError:
            pass
        if proc.poll() is None:
            proc.kill()
            proc.wait()

    return proc.returncode, output.decode("utf-8", errors="replace")


def _run_cli_tty(args: list[str], cwd: Path, keys: bytes) -> tuple[int, str]:
    return _run_cli_tty_script(args, cwd, [(b"Select deps:", keys)])


def _clean_cli_output(output: str) -> str:
    return ANSI_ESCAPE_RE.sub("", output).replace("\r\n", "\n").replace("\r", "\n")


def _compact_cli_output(output: str) -> str:
    return "\n".join(
        re.sub(r"[ \t]+", " ", line).rstrip()
        for line in _clean_cli_output(output).splitlines()
    )


def _nonempty_cli_lines(output: str) -> list[str]:
    return [line.strip() for line in _compact_cli_output(output).splitlines() if line.strip()]


def _search_result_lines(output: str) -> list[str]:
    return [line for line in _nonempty_cli_lines(output) if SEARCH_RESULT_RE.match(line)]


def _chain_item_lines(output: str) -> list[str]:
    return [line for line in _nonempty_cli_lines(output) if CHAIN_ITEM_RE.match(line)]


def _eval_block_lines(output: str) -> list[str]:
    return [line for line in _nonempty_cli_lines(output) if EVAL_BLOCK_RE.match(line)]


def _extract_created_mdoc(output: str) -> tuple[str, str]:
    fnode_match = re.search(
        r"^fnode:\s*([0-9a-fA-F-]+)$", output, re.MULTILINE)
    path_match = re.search(r"^created:\s*(.+\.mdoc)$", output, re.MULTILINE)
    if not fnode_match or not path_match:
        raise AssertionError(f"Failed to parse creation output:\n{output}")
    return fnode_match.group(1), path_match.group(1)


def _rewrite_title(mdoc_path: str, new_title: str) -> None:
    path = Path(mdoc_path)
    content = path.read_text(encoding="utf-8")
    updated = re.sub(
        r"(?m)^@title:\s*.*$",
        f"@title: {new_title}",
        content,
        count=1,
    )
    if updated == content:
        raise AssertionError(f"Failed to rewrite title for {mdoc_path}")
    path.write_text(updated, encoding="utf-8")


def _append_src_block(mdoc_path: str, srctype: str, body: str) -> None:
    path = Path(mdoc_path)
    text = path.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    text += f"@src: {srctype}\n"
    if body:
        for line in body.splitlines():
            text += f"{line}\n"
    text += "@end\n\n"
    path.write_text(text, encoding="utf-8")


def _rename_mdoc(mdoc_path: str, new_name: str) -> str:
    path = Path(mdoc_path)
    target = path.with_name(new_name)
    path.rename(target)
    return str(target)


def _make_mdoc_invalid(mdoc_path: str) -> None:
    path = Path(mdoc_path)
    text = path.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    text += "@title: Duplicate Broken Title\n"
    path.write_text(text, encoding="utf-8")


class TestMdcCli(unittest.TestCase):
    def test_init_new_search_and_path_boundary(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_basic.") as tmp:
            repo = Path(tmp)

            init_1 = _run_cli(["init"], repo)
            self.assertEqual(init_1.returncode, 0,
                             init_1.stdout + init_1.stderr)
            self.assertTrue((repo / ".mdc").is_dir())
            self.assertTrue((repo / ".mdc" / "config.toml").is_file())
            config_text = (repo / ".mdc" /
                           "config.toml").read_text(encoding="utf-8")
            self.assertEqual(config_text, "")

            init_2 = _run_cli(["init"], repo)
            self.assertEqual(init_2.returncode, 0,
                             init_2.stdout + init_2.stderr)
            self.assertIn("Already initialized", init_2.stdout)

            new_1 = _run_cli(["new", "-t", "Linear Algebra"], repo)
            self.assertEqual(new_1.returncode, 0, new_1.stdout + new_1.stderr)
            fnode_1, path_1 = _extract_created_mdoc(new_1.stdout)
            self.assertTrue(Path(path_1).is_file())
            self.assertEqual(Path(path_1).name, f"{fnode_1}.mdoc")

            new_2 = _run_cli(
                ["new", "-t", "Matrix Rank", "-f", "notes/matrix-rank"], repo
            )
            self.assertEqual(new_2.returncode, 0, new_2.stdout + new_2.stderr)
            _, path_2 = _extract_created_mdoc(new_2.stdout)
            self.assertTrue(Path(path_2).is_file())
            self.assertEqual(
                Path(path_2).relative_to(repo.resolve()).as_posix(),
                "notes/matrix-rank.mdoc",
            )

            new_3 = _run_cli(
                ["new", "-t", "Matrix Factorization", "-f", "notes/factor.mdoc"], repo
            )
            self.assertEqual(new_3.returncode, 0, new_3.stdout + new_3.stderr)
            _, path_3 = _extract_created_mdoc(new_3.stdout)
            self.assertTrue(Path(path_3).is_file())
            self.assertEqual(
                Path(path_3).relative_to(repo.resolve()).as_posix(),
                "notes/factor.mdoc.mdoc",
            )

            outside = repo.parent / "outside"
            outside.mkdir(parents=True, exist_ok=True)
            new_outside = _run_cli(
                ["new", "-t", "Bad", "-f", str(outside)], repo)
            self.assertEqual(new_outside.returncode, 1)
            self.assertIn("target path must be relative to the mdoc root",
                          new_outside.stdout)

            search_title = _run_cli(["search", "matrix"], repo)
            self.assertEqual(search_title.returncode, 0,
                             search_title.stdout + search_title.stderr)
            search_title_text = _compact_cli_output(search_title.stdout)
            self.assertEqual(len(_search_result_lines(search_title.stdout)), 2)
            self.assertRegex(
                search_title_text,
                r"(?m)^- [0-9a-f]{8} Matrix Rank \(.+\.mdoc\)$",
            )
            self.assertIn("Matrix Factorization", search_title.stdout)

            search_limited = _run_cli(
                ["search", "matrix", "--max-results", "1"], repo
            )
            self.assertEqual(
                search_limited.returncode,
                0,
                search_limited.stdout + search_limited.stderr,
            )
            self.assertEqual(len(_search_result_lines(search_limited.stdout)), 1)

            search_fnode = _run_cli(["search", fnode_1[:8]], repo)
            self.assertEqual(search_fnode.returncode, 0,
                             search_fnode.stdout + search_fnode.stderr)
            self.assertIn("Linear Algebra", search_fnode.stdout)

    def test_eval_runs_natl_and_py_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_ok.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Eval Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            _append_src_block(src_path, "natl", "hello from natl")
            _append_src_block(src_path, "py", "print('hello from py')")

            eval_run = _run_cli(["eval", src_path], repo)
            self.assertEqual(eval_run.returncode, 0,
                             eval_run.stdout + eval_run.stderr)
            self.assertEqual(len(_eval_block_lines(eval_run.stdout)), 2)
            self.assertIn("[1] natl: ok", eval_run.stdout)
            self.assertIn("hello from natl", eval_run.stdout)
            self.assertIn("[2] py: ok", eval_run.stdout)
            self.assertIn("hello from py", eval_run.stdout)
            self.assertFalse(
                any("failed" in line for line in _eval_block_lines(eval_run.stdout))
            )

    def test_eval_returns_nonzero_when_py_block_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_fail.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(
                ["new", "-t", "Eval Failing Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            _append_src_block(src_path, "py", "raise RuntimeError('boom')")

            eval_run = _run_cli(["eval", src_path], repo)
            self.assertEqual(eval_run.returncode, 1)
            self.assertEqual(len(_eval_block_lines(eval_run.stdout)), 1)
            self.assertIn("[1] py: failed (1)", eval_run.stdout)
            self.assertIn("boom", eval_run.stdout)
            self.assertEqual(
                sum("failed" in line for line in _eval_block_lines(eval_run.stdout)),
                1,
            )

    def test_eval_respects_per_srctype_reverse_depens_config(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_depth_reverse.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_root = _run_cli(["new", "-t", "Root Card", "-f", "."], repo)
            self.assertEqual(new_root.returncode, 0,
                             new_root.stdout + new_root.stderr)
            _, root_path = _extract_created_mdoc(new_root.stdout)

            new_dep1 = _run_cli(["new", "-t", "Dep One", "-f", "."], repo)
            self.assertEqual(new_dep1.returncode, 0,
                             new_dep1.stdout + new_dep1.stderr)
            _, dep1_path = _extract_created_mdoc(new_dep1.stdout)

            new_dep2 = _run_cli(["new", "-t", "Dep Two", "-f", "."], repo)
            self.assertEqual(new_dep2.returncode, 0,
                             new_dep2.stdout + new_dep2.stderr)
            _, dep2_path = _extract_created_mdoc(new_dep2.stdout)

            _append_src_block(root_path, "natl", "root-body")
            _append_src_block(dep1_path, "natl", "dep1-body")
            _append_src_block(dep2_path, "natl", "dep2-body")

            rc_add_root, out_add_root = _run_cli_tty(
                ["dep", "add", root_path, "Dep One"], repo, b" \r"
            )
            self.assertEqual(rc_add_root, 0, out_add_root)
            self.assertIn("added: 1", _compact_cli_output(out_add_root))

            rc_add_dep1, out_add_dep1 = _run_cli_tty(
                ["dep", "add", dep1_path, "Dep Two"], repo, b" \r"
            )
            self.assertEqual(rc_add_dep1, 0, out_add_dep1)
            self.assertIn("added: 1", _compact_cli_output(out_add_dep1))

            eval_depth_1 = _run_cli(["eval", "-d", "1", root_path], repo)
            self.assertEqual(eval_depth_1.returncode, 0,
                             eval_depth_1.stdout + eval_depth_1.stderr)
            self.assertIn("dep1-body", eval_depth_1.stdout)
            self.assertIn("root-body", eval_depth_1.stdout)
            self.assertNotIn("dep2-body", eval_depth_1.stdout)

            eval_depth_inf = _run_cli(
                ["eval", "--depth", "-1", root_path], repo)
            self.assertEqual(eval_depth_inf.returncode, 0,
                             eval_depth_inf.stdout + eval_depth_inf.stderr)
            self.assertIn("dep2-body", eval_depth_inf.stdout)
            self.assertIn("dep1-body", eval_depth_inf.stdout)
            self.assertIn("root-body", eval_depth_inf.stdout)
            root_pos = eval_depth_inf.stdout.find("root-body")
            dep1_pos = eval_depth_inf.stdout.find("dep1-body")
            dep2_pos = eval_depth_inf.stdout.find("dep2-body")
            self.assertGreaterEqual(root_pos, 0, eval_depth_inf.stdout)
            self.assertGreaterEqual(dep1_pos, 0, eval_depth_inf.stdout)
            self.assertGreaterEqual(dep2_pos, 0, eval_depth_inf.stdout)
            self.assertLess(root_pos, dep1_pos, eval_depth_inf.stdout)
            self.assertLess(dep1_pos, dep2_pos, eval_depth_inf.stdout)

            (repo / ".mdc" / "config.toml").write_text(
                "[src.natl]\nreverse_depens = false\n",
                encoding="utf-8",
            )

            eval_topological = _run_cli(
                ["eval", "--depth", "-1", root_path], repo)
            self.assertEqual(eval_topological.returncode, 0,
                             eval_topological.stdout + eval_topological.stderr)
            root_pos = eval_topological.stdout.find("root-body")
            dep1_pos = eval_topological.stdout.find("dep1-body")
            dep2_pos = eval_topological.stdout.find("dep2-body")
            self.assertGreaterEqual(root_pos, 0, eval_topological.stdout)
            self.assertGreaterEqual(dep1_pos, 0, eval_topological.stdout)
            self.assertGreaterEqual(dep2_pos, 0, eval_topological.stdout)
            self.assertLess(dep2_pos, dep1_pos, eval_topological.stdout)
            self.assertLess(dep1_pos, root_pos, eval_topological.stdout)

    def test_eval_runs_latex_block_with_xelatex(self) -> None:
        required = ("latexmk", "xelatex")
        missing = [name for name in required if shutil.which(name) is None]
        if missing:
            self.skipTest(f"missing tools: {', '.join(missing)}")

        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_latex.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(
                ["new", "-t", "Eval Latex Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            src_fnode, src_path = _extract_created_mdoc(new_src.stdout)

            (repo / ".mdc" / "config.toml").write_text(
                "[src.latex]\n"
                "preamble = '''\n"
                "\\documentclass[varwidth=true, border=5pt, crop]{standalone}\n"
                "\\newcommand{\\MDCMARK}{MARK}\n"
                "\\begin{document}\n"
                "'''\n"
                "postamble = '''\n"
                "\\end{document}\n"
                "% cfg-tail\n"
                "'''\n",
                encoding="utf-8",
            )

            _append_src_block(src_path, "latex",
                              r"$\int_0^1 x^2\,dx=\frac{1}{3}$")

            eval_run = _run_cli(["eval", src_path], repo)
            self.assertEqual(eval_run.returncode, 0,
                             eval_run.stdout + eval_run.stderr)
            self.assertIn("[1] latex: ok", eval_run.stdout)
            self.assertIn("temp-latex-", eval_run.stdout)
            self.assertIn("artifact dir:", eval_run.stdout)
            self.assertIn("artifact tex:", eval_run.stdout)
            self.assertIn("artifact pdf:", eval_run.stdout)
            tex_match = re.search(
                r"(?m)^\s+artifact tex:\s*(.+\.tex)$", eval_run.stdout)
            self.assertIsNotNone(tex_match, eval_run.stdout)
            tex_path = Path(tex_match.group(1))
            self.assertTrue(tex_path.is_file())
            tex_payload = tex_path.read_text(encoding="utf-8")
            self.assertIn("\\newcommand{\\MDCMARK}{MARK}", tex_payload)
            self.assertIn("% cfg-tail", tex_payload)

    def test_dep_add_show_rm_interactive(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Card", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            src_fnode, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Matrix Rank", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0,
                             new_dep.stdout + new_dep.stderr)
            dep_fnode, dep_path = _extract_created_mdoc(new_dep.stdout)
            dep_rel = str(Path(dep_path).relative_to(
                repo.resolve())).replace("\\", "/")

            show_0 = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_0.returncode, 0,
                             show_0.stdout + show_0.stderr)
            self.assertEqual(len(_chain_item_lines(show_0.stdout)), 0)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "matrix"], repo, b" \r")
            self.assertEqual(rc_add, 0, out_add)
            out_add_text = _compact_cli_output(out_add)
            self.assertIn("added: 1", out_add_text)
            self.assertRegex(
                out_add_text,
                rf"\+ {dep_fnode[:8]} Matrix Rank \({re.escape(dep_rel)}\)",
            )
            self.assertRegex(
                out_add_text,
                rf"source: {src_fnode[:8]} Source Card",
            )

            show_1 = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_1.returncode, 0,
                             show_1.stdout + show_1.stderr)
            show_1_text = _compact_cli_output(show_1.stdout)
            self.assertEqual(len(_chain_item_lines(show_1.stdout)), 1)
            self.assertRegex(
                show_1_text,
                rf"\[1\] - {dep_fnode[:8]} Matrix Rank \({re.escape(dep_rel)}\)",
            )

            rc_add_dup, out_add_dup = _run_cli_tty(
                ["dep", "add", src_path, "matrix"], repo, b" \r")
            self.assertEqual(rc_add_dup, 0, out_add_dup)
            out_add_dup_text = _compact_cli_output(out_add_dup)
            self.assertIn("added: 0", out_add_dup_text)
            self.assertIn("skipped existing: 1", out_add_dup_text)

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            out_rm_text = _compact_cli_output(out_rm)
            self.assertIn("removed: 1", out_rm_text)
            self.assertRegex(
                out_rm_text,
                rf"- {dep_fnode[:8]} Matrix Rank \({re.escape(dep_rel)}\)",
            )

            show_2 = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_2.returncode, 0,
                             show_2.stdout + show_2.stderr)
            self.assertEqual(len(_chain_item_lines(show_2.stdout)), 0)

    def test_dep_add_can_create_missing_dependency_interactively(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_add_create.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Card", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            rc_add, out_add = _run_cli_tty_script(
                ["dep", "add", src_path, "Fresh Dep"],
                repo,
                [
                    (b"[y/N]:", b"y\r"),
                    (b"Filename [", b"notes/fresh-dep\r"),
                    (b"Title [Untitled]:", b"Fresh Dep\r"),
                ],
            )
            self.assertEqual(rc_add, 0, out_add)
            out_add_text = _compact_cli_output(out_add)
            self.assertIn("added: 1", out_add_text)
            self.assertIn("Fresh Dep", out_add_text)
            self.assertIn("notes/fresh-dep.mdoc", out_add_text)
            self.assertRegex(out_add_text, r"Filename \[[0-9a-f]{8}\.\.\.\]:")
            self.assertIn("\x1b[3A", out_add)
            self.assertIn("\x1b[3M", out_add)

            created_path = repo / "notes" / "fresh-dep.mdoc"
            self.assertTrue(created_path.is_file())

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            self.assertEqual(len(_chain_item_lines(show_run.stdout)), 1)
            self.assertIn("Fresh Dep", show_run.stdout)
            self.assertIn("notes/fresh-dep.mdoc", show_run.stdout)

            search_run = _run_cli(["search", "Fresh Dep"], repo)
            self.assertEqual(search_run.returncode, 0, search_run.stdout + search_run.stderr)
            self.assertEqual(len(_search_result_lines(search_run.stdout)), 1)
            self.assertIn("Fresh Dep", search_run.stdout)

    def test_nested_repos_are_fully_isolated(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_nested.") as tmp:
            parent = Path(tmp) / "parent"
            child = parent / "child"
            parent.mkdir(parents=True)
            child.mkdir(parents=True)

            self.assertEqual(_run_cli(["init"], parent).returncode, 0)
            self.assertEqual(
                _run_cli(["new", "-t", "Parent Only", "-f", "."], parent).returncode, 0)

            self.assertEqual(_run_cli(["init"], child).returncode, 0)
            child_new = _run_cli(["new", "-t", "Child Only", "-f", "."], child)
            self.assertEqual(child_new.returncode, 0, child_new.stdout + child_new.stderr)
            _, child_path = _extract_created_mdoc(child_new.stdout)

            parent_search_child = _run_cli(["search", "Child Only"], parent)
            self.assertEqual(
                parent_search_child.returncode, 0, parent_search_child.stdout +
                parent_search_child.stderr
            )
            self.assertIn("No results for: Child Only", parent_search_child.stdout)

            child_search_parent = _run_cli(["search", "Parent Only"], child)
            self.assertEqual(
                child_search_parent.returncode, 0, child_search_parent.stdout +
                child_search_parent.stderr
            )
            self.assertIn("No results for: Parent Only",
                          child_search_parent.stdout)

            parent_graph = _run_cli(["graph", "check"], parent)
            self.assertEqual(parent_graph.returncode, 0, parent_graph.stdout + parent_graph.stderr)
            self.assertIn("nodes: 1", _compact_cli_output(parent_graph.stdout))

            child_graph = _run_cli(["graph", "check"], child)
            self.assertEqual(child_graph.returncode, 0, child_graph.stdout + child_graph.stderr)
            self.assertIn("nodes: 1", _compact_cli_output(child_graph.stdout))

            parent_show_child = _run_cli(["dep", "show", child_path], parent)
            self.assertEqual(
                parent_show_child.returncode,
                1,
                parent_show_child.stdout + parent_show_child.stderr,
            )
            self.assertIn("nested mdoc root", parent_show_child.stdout)

    def test_dep_show_refreshes_accessed_dependency_index(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_show_refresh.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(
                ["new", "-t", "Source For Show", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Show Dep Old", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0,
                             new_dep.stdout + new_dep.stderr)
            _, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Show Dep Old"], repo, b" \r")
            self.assertEqual(rc_add, 0, out_add)
            self.assertIn("added: 1", _compact_cli_output(out_add))

            _rewrite_title(dep_path, "Show Dep New")

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0,
                             show_run.stdout + show_run.stderr)
            self.assertIn("Show Dep New", show_run.stdout)

            search_new = _run_cli(["search", "Show Dep New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertIn("Show Dep New", search_new.stdout)

    def test_dep_show_accepts_depth_and_detects_cycles(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_show_depth.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source For Depth", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            src_fnode, src_path = _extract_created_mdoc(new_src.stdout)
            src_rel = str(Path(src_path).relative_to(repo.resolve())).replace("\\", "/")

            new_dep1 = _run_cli(["new", "-t", "Depth One", "-f", "."], repo)
            self.assertEqual(new_dep1.returncode, 0, new_dep1.stdout + new_dep1.stderr)
            dep1_fnode, dep1_path = _extract_created_mdoc(new_dep1.stdout)
            dep1_rel = str(Path(dep1_path).relative_to(repo.resolve())).replace("\\", "/")

            new_dep2 = _run_cli(["new", "-t", "Depth Two", "-f", "."], repo)
            self.assertEqual(new_dep2.returncode, 0, new_dep2.stdout + new_dep2.stderr)
            dep2_fnode, dep2_path = _extract_created_mdoc(new_dep2.stdout)
            dep2_rel = str(Path(dep2_path).relative_to(repo.resolve())).replace("\\", "/")

            rc_add_src, out_add_src = _run_cli_tty(
                ["dep", "add", src_path, "Depth One"], repo, b" \r"
            )
            self.assertEqual(rc_add_src, 0, out_add_src)

            rc_add_dep1, out_add_dep1 = _run_cli_tty(
                ["dep", "add", dep1_path, "Depth Two"], repo, b" \r"
            )
            self.assertEqual(rc_add_dep1, 0, out_add_dep1)

            show_default = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_default.returncode, 0, show_default.stdout + show_default.stderr)
            show_default_text = _compact_cli_output(show_default.stdout)
            self.assertEqual(len(_chain_item_lines(show_default.stdout)), 1)
            self.assertRegex(
                show_default_text,
                rf"\[1\] - {dep1_fnode[:8]} Depth One \({re.escape(dep1_rel)}\)",
            )
            self.assertNotIn(dep2_fnode[:8], show_default_text)

            show_inf = _run_cli(["dep", "show", "--depth", "-1", src_path], repo)
            self.assertEqual(show_inf.returncode, 0, show_inf.stdout + show_inf.stderr)
            show_inf_text = _compact_cli_output(show_inf.stdout)
            self.assertEqual(len(_chain_item_lines(show_inf.stdout)), 2)
            self.assertRegex(
                show_inf_text,
                rf"\[1\] - {dep1_fnode[:8]} Depth One \({re.escape(dep1_rel)}\)",
            )
            self.assertRegex(
                show_inf_text,
                rf"\[2\] - {dep2_fnode[:8]} Depth Two \({re.escape(dep2_rel)}\)",
            )

            rc_add_cycle, out_add_cycle = _run_cli_tty(
                ["dep", "add", dep2_path, src_fnode[:8]], repo, b" \r"
            )
            self.assertEqual(rc_add_cycle, 0, out_add_cycle)

            show_cycle = _run_cli(["dep", "show", "--depth", "-1", src_path], repo)
            combined = show_cycle.stdout + show_cycle.stderr
            self.assertEqual(show_cycle.returncode, 1, combined)
            self.assertIn("dependency cycle detected", combined)
            self.assertIn(src_fnode, combined)
            self.assertIn(dep1_fnode, combined)
            self.assertIn(dep2_fnode, combined)

    def test_dep_show_warns_but_continues_on_missing_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_show_missing.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Missing", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Gone Dep", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            dep_fnode, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Gone Dep"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            Path(dep_path).unlink()

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            show_run_text = _compact_cli_output(show_run.stdout)
            self.assertEqual(len(_chain_item_lines(show_run.stdout)), 1)
            self.assertIn(f"[1] - {dep_fnode[:8]}", show_run_text)
            self.assertIn("<missing> (<unknown>)", show_run_text)
            self.assertIn("Warning: detected 1 broken dependency reference", show_run_text)

    def test_dep_rm_can_remove_missing_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_rm_missing.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Missing Rm", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Gone Dep Rm", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            _, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Gone Dep Rm"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            Path(dep_path).unlink()

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            out_rm_text = _compact_cli_output(out_rm)
            self.assertIn("<missing>", out_rm_text)
            self.assertIn("removed: 1", out_rm_text)

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            self.assertEqual(len(_chain_item_lines(show_run.stdout)), 0)

    def test_eval_prints_dependency_chain_and_blocks_on_missing_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_missing.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Eval Missing Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)
            _append_src_block(src_path, "natl", "root body")

            new_dep = _run_cli(["new", "-t", "Eval Missing Dep", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            dep_fnode, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Eval Missing Dep"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            Path(dep_path).unlink()

            eval_run = _run_cli(["eval", src_path], repo)
            self.assertEqual(eval_run.returncode, 1, eval_run.stdout + eval_run.stderr)
            self.assertEqual(len(_chain_item_lines(eval_run.stdout)), 1)
            self.assertIn(dep_fnode[:8], eval_run.stdout)
            self.assertIn("<missing>", eval_run.stdout)
            self.assertIn("remove the broken references with `mdc dep rm` before eval", eval_run.stdout)
            self.assertEqual(len(_eval_block_lines(eval_run.stdout)), 0)

    def test_dep_show_warns_but_continues_on_invalid_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_show_invalid.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Invalid", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Broken Dep", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            dep_fnode, dep_path = _extract_created_mdoc(new_dep.stdout)
            dep_rel = str(Path(dep_path).relative_to(repo.resolve())).replace("\\", "/")

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Broken Dep"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            _make_mdoc_invalid(dep_path)

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            show_run_text = _compact_cli_output(show_run.stdout)
            self.assertEqual(len(_chain_item_lines(show_run.stdout)), 1)
            self.assertIn(dep_fnode[:8], show_run_text)
            self.assertIn(f"<invalid> ({dep_rel})", show_run_text)
            self.assertIn("Warning: detected 1 broken dependency reference(s) (1 invalid)", show_run_text)

    def test_dep_rm_can_remove_invalid_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_rm_invalid.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Invalid Rm", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Broken Dep Rm", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            _, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Broken Dep Rm"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            _make_mdoc_invalid(dep_path)

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            out_rm_text = _compact_cli_output(out_rm)
            self.assertIn("<invalid>", out_rm_text)
            self.assertIn("removed: 1", out_rm_text)

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            self.assertEqual(len(_chain_item_lines(show_run.stdout)), 0)

    def test_eval_prints_dependency_chain_and_blocks_on_invalid_dependency(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_eval_invalid.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Eval Invalid Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)
            _append_src_block(src_path, "natl", "root body")

            new_dep = _run_cli(["new", "-t", "Eval Invalid Dep", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            dep_fnode, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Eval Invalid Dep"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            _make_mdoc_invalid(dep_path)

            eval_run = _run_cli(["eval", src_path], repo)
            self.assertEqual(eval_run.returncode, 1, eval_run.stdout + eval_run.stderr)
            self.assertEqual(len(_chain_item_lines(eval_run.stdout)), 1)
            self.assertIn(dep_fnode[:8], eval_run.stdout)
            self.assertIn("<invalid>", eval_run.stdout)
            self.assertIn("remove the broken references with `mdc dep rm` before eval", eval_run.stdout)
            self.assertEqual(len(_eval_block_lines(eval_run.stdout)), 0)

    def test_graph_check_reports_missing_invalid_and_cycle(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_graph_check.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_bad = _run_cli(["new", "-t", "Broken Graph Node", "-f", "."], repo)
            self.assertEqual(new_bad.returncode, 0, new_bad.stdout + new_bad.stderr)
            bad_fnode, bad_path = _extract_created_mdoc(new_bad.stdout)
            bad_rel = str(Path(bad_path).relative_to(repo.resolve())).replace("\\", "/")
            _make_mdoc_invalid(bad_path)

            new_a = _run_cli(["new", "-t", "Cycle A", "-f", "."], repo)
            self.assertEqual(new_a.returncode, 0, new_a.stdout + new_a.stderr)
            a_fnode, a_path = _extract_created_mdoc(new_a.stdout)

            new_b = _run_cli(["new", "-t", "Cycle B", "-f", "."], repo)
            self.assertEqual(new_b.returncode, 0, new_b.stdout + new_b.stderr)
            b_fnode, b_path = _extract_created_mdoc(new_b.stdout)

            new_src = _run_cli(["new", "-t", "Graph Source", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            rc_add_bad, out_add_bad = _run_cli_tty(
                ["dep", "add", src_path, "Broken Graph Node"], repo, b" \r"
            )
            self.assertEqual(rc_add_bad, 0, out_add_bad)

            src_text = Path(src_path).read_text(encoding="utf-8")
            Path(src_path).write_text(
                src_text.replace("@end", "missing-target-001\n@end", 1),
                encoding="utf-8",
            )

            rc_add_a, out_add_a = _run_cli_tty(
                ["dep", "add", a_path, "Cycle B"], repo, b" \r"
            )
            self.assertEqual(rc_add_a, 0, out_add_a)

            rc_add_b, out_add_b = _run_cli_tty(
                ["dep", "add", b_path, "Cycle A"], repo, b" \r"
            )
            self.assertEqual(rc_add_b, 0, out_add_b)

            check_run = _run_cli(["graph", "check"], repo)
            self.assertEqual(check_run.returncode, 1, check_run.stdout + check_run.stderr)
            check_run_text = _compact_cli_output(check_run.stdout)
            self.assertIn("nodes: 4", check_run_text)
            self.assertIn("edges: 4", check_run_text)
            self.assertIn("missing: 1", check_run_text)
            self.assertIn("invalid: 1", check_run_text)
            self.assertIn("cycles: 1", check_run_text)
            self.assertIn("missing-target-001", check_run_text)
            self.assertIn(f"<invalid> ({bad_rel})", check_run_text)
            self.assertIn("dependency cycle detected", check_run_text)
            self.assertIn(a_fnode[:8], check_run_text)
            self.assertIn(b_fnode[:8], check_run_text)
            self.assertIn(bad_fnode[:8], check_run_text)

    def test_dep_refs_reports_reverse_dependencies(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_refs.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_root = _run_cli(["new", "-t", "Root Ref", "-f", "."], repo)
            self.assertEqual(new_root.returncode, 0, new_root.stdout + new_root.stderr)
            root_fnode, root_path = _extract_created_mdoc(new_root.stdout)

            new_mid1 = _run_cli(["new", "-t", "Mid One", "-f", "."], repo)
            self.assertEqual(new_mid1.returncode, 0, new_mid1.stdout + new_mid1.stderr)
            mid1_fnode, mid1_path = _extract_created_mdoc(new_mid1.stdout)

            new_mid2 = _run_cli(["new", "-t", "Mid Two", "-f", "."], repo)
            self.assertEqual(new_mid2.returncode, 0, new_mid2.stdout + new_mid2.stderr)
            mid2_fnode, mid2_path = _extract_created_mdoc(new_mid2.stdout)

            new_leaf = _run_cli(["new", "-t", "Leaf Ref", "-f", "."], repo)
            self.assertEqual(new_leaf.returncode, 0, new_leaf.stdout + new_leaf.stderr)
            leaf_fnode, leaf_path = _extract_created_mdoc(new_leaf.stdout)

            rc_add_root_1, out_add_root_1 = _run_cli_tty(
                ["dep", "add", root_path, "Mid One"], repo, b" \r"
            )
            self.assertEqual(rc_add_root_1, 0, out_add_root_1)

            rc_add_root_2, out_add_root_2 = _run_cli_tty(
                ["dep", "add", root_path, "Mid Two"], repo, b" \r"
            )
            self.assertEqual(rc_add_root_2, 0, out_add_root_2)

            rc_add_mid1, out_add_mid1 = _run_cli_tty(
                ["dep", "add", mid1_path, "Leaf Ref"], repo, b" \r"
            )
            self.assertEqual(rc_add_mid1, 0, out_add_mid1)

            rc_add_mid2, out_add_mid2 = _run_cli_tty(
                ["dep", "add", mid2_path, "Leaf Ref"], repo, b" \r"
            )
            self.assertEqual(rc_add_mid2, 0, out_add_mid2)

            refs_run = _run_cli(["dep", "refs", "--depth", "-1", leaf_path], repo)
            self.assertEqual(refs_run.returncode, 0, refs_run.stdout + refs_run.stderr)
            refs_run_text = _compact_cli_output(refs_run.stdout)
            self.assertIn(f"target: {leaf_fnode[:8]}", refs_run_text)
            self.assertIn("Leaf Ref", refs_run_text)
            self.assertEqual(len(_chain_item_lines(refs_run.stdout)), 3)
            self.assertRegex(refs_run_text, rf"\[1\] - {mid1_fnode[:8]} Mid One")
            self.assertRegex(refs_run_text, rf"\[1\] - {mid2_fnode[:8]} Mid Two")
            self.assertRegex(refs_run_text, rf"\[2\] - {root_fnode[:8]} Root Ref")

    def test_dep_leaf_reports_all_reachable_leaf_dependencies(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_leaf.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_root = _run_cli(["new", "-t", "Root Leaf", "-f", "."], repo)
            self.assertEqual(new_root.returncode, 0, new_root.stdout + new_root.stderr)
            root_fnode, root_path = _extract_created_mdoc(new_root.stdout)

            new_mid1 = _run_cli(["new", "-t", "Mid Leaf One", "-f", "."], repo)
            self.assertEqual(new_mid1.returncode, 0, new_mid1.stdout + new_mid1.stderr)
            mid1_fnode, mid1_path = _extract_created_mdoc(new_mid1.stdout)

            new_mid2 = _run_cli(["new", "-t", "Mid Leaf Two", "-f", "."], repo)
            self.assertEqual(new_mid2.returncode, 0, new_mid2.stdout + new_mid2.stderr)
            mid2_fnode, mid2_path = _extract_created_mdoc(new_mid2.stdout)

            new_direct = _run_cli(["new", "-t", "Leaf Direct", "-f", "."], repo)
            self.assertEqual(new_direct.returncode, 0, new_direct.stdout + new_direct.stderr)
            direct_fnode, direct_path = _extract_created_mdoc(new_direct.stdout)

            new_shared = _run_cli(["new", "-t", "Leaf Shared", "-f", "."], repo)
            self.assertEqual(new_shared.returncode, 0, new_shared.stdout + new_shared.stderr)
            shared_fnode, shared_path = _extract_created_mdoc(new_shared.stdout)

            new_other = _run_cli(["new", "-t", "Leaf Other", "-f", "."], repo)
            self.assertEqual(new_other.returncode, 0, new_other.stdout + new_other.stderr)
            other_fnode, other_path = _extract_created_mdoc(new_other.stdout)

            rc_add_root_1, out_add_root_1 = _run_cli_tty(
                ["dep", "add", root_path, "Mid Leaf One"], repo, b" \r"
            )
            self.assertEqual(rc_add_root_1, 0, out_add_root_1)

            rc_add_root_2, out_add_root_2 = _run_cli_tty(
                ["dep", "add", root_path, "Leaf Direct"], repo, b" \r"
            )
            self.assertEqual(rc_add_root_2, 0, out_add_root_2)

            rc_add_root_3, out_add_root_3 = _run_cli_tty(
                ["dep", "add", root_path, "Mid Leaf Two"], repo, b" \r"
            )
            self.assertEqual(rc_add_root_3, 0, out_add_root_3)

            rc_add_mid1, out_add_mid1 = _run_cli_tty(
                ["dep", "add", mid1_path, "Leaf Shared"], repo, b" \r"
            )
            self.assertEqual(rc_add_mid1, 0, out_add_mid1)

            rc_add_mid2_1, out_add_mid2_1 = _run_cli_tty(
                ["dep", "add", mid2_path, "Leaf Shared"], repo, b" \r"
            )
            self.assertEqual(rc_add_mid2_1, 0, out_add_mid2_1)

            rc_add_mid2_2, out_add_mid2_2 = _run_cli_tty(
                ["dep", "add", mid2_path, "Leaf Other"], repo, b" \r"
            )
            self.assertEqual(rc_add_mid2_2, 0, out_add_mid2_2)

            leaf_run = _run_cli(["dep", "leaf", root_path], repo)
            self.assertEqual(leaf_run.returncode, 0, leaf_run.stdout + leaf_run.stderr)
            leaf_run_text = _compact_cli_output(leaf_run.stdout)
            self.assertIn(f"source: {root_fnode[:8]}", leaf_run_text)
            self.assertIn("Root Leaf", leaf_run_text)
            self.assertEqual(len(_chain_item_lines(leaf_run.stdout)), 3)
            self.assertRegex(leaf_run_text, rf"\[1\] - {direct_fnode[:8]} Leaf Direct")
            self.assertRegex(leaf_run_text, rf"\[2\] - {shared_fnode[:8]} Leaf Shared")
            self.assertRegex(leaf_run_text, rf"\[2\] - {other_fnode[:8]} Leaf Other")
            self.assertNotIn(mid1_fnode[:8], leaf_run_text)
            self.assertNotIn(mid2_fnode[:8], leaf_run_text)
            self.assertEqual(
                len(
                    re.findall(
                        rf"\[2\] - {shared_fnode[:8]} Leaf Shared",
                        leaf_run_text,
                    )
                ),
                1,
            )
            self.assertIn(Path(direct_path).name, leaf_run.stdout)
            self.assertIn(Path(shared_path).name, leaf_run.stdout)
            self.assertIn(Path(other_path).name, leaf_run.stdout)

    def test_dep_show_recovers_after_manual_file_rename(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_show_rename.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source Rename", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0, new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            new_dep = _run_cli(["new", "-t", "Rename Dep", "-f", "."], repo)
            self.assertEqual(new_dep.returncode, 0, new_dep.stdout + new_dep.stderr)
            _, dep_path = _extract_created_mdoc(new_dep.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Rename Dep"], repo, b" \r"
            )
            self.assertEqual(rc_add, 0, out_add)

            renamed_path = _rename_mdoc(dep_path, "renamed-dep.mdoc")
            renamed_rel = str(Path(renamed_path).relative_to(repo.resolve())).replace("\\", "/")

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0, show_run.stdout + show_run.stderr)
            self.assertIn("Rename Dep", show_run.stdout)
            self.assertIn(renamed_rel, show_run.stdout)

    def test_dep_add_refreshes_accessed_dependency_index(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_add_refresh.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(
                ["new", "-t", "Source For Add", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            dep1_new = _run_cli(["new", "-t", "Add Old One", "-f", "."], repo)
            self.assertEqual(dep1_new.returncode, 0,
                             dep1_new.stdout + dep1_new.stderr)
            _, dep1_path = _extract_created_mdoc(dep1_new.stdout)

            dep2_new = _run_cli(["new", "-t", "Add Old Two", "-f", "."], repo)
            self.assertEqual(dep2_new.returncode, 0,
                             dep2_new.stdout + dep2_new.stderr)
            _, dep2_path = _extract_created_mdoc(dep2_new.stdout)

            _rewrite_title(dep1_path, "Add New One")
            _rewrite_title(dep2_path, "Add New Two")

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "Add Old"], repo, b" \r")
            self.assertEqual(rc_add, 0, out_add)
            out_add_text = _compact_cli_output(out_add)
            self.assertIn("added: 1", out_add_text)
            self.assertRegex(out_add_text, r"Add New (One|Two)")

            search_new = _run_cli(["search", "Add New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertEqual(len(_search_result_lines(search_new.stdout)), 1)
            self.assertTrue(
                ("Add New One" in search_new.stdout) ^ (
                    "Add New Two" in search_new.stdout),
                search_new.stdout,
            )

    def test_dep_rm_refreshes_accessed_dependency_index(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_dep_rm_refresh.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            new_src = _run_cli(["new", "-t", "Source For Rm", "-f", "."], repo)
            self.assertEqual(new_src.returncode, 0,
                             new_src.stdout + new_src.stderr)
            _, src_path = _extract_created_mdoc(new_src.stdout)

            dep1_new = _run_cli(["new", "-t", "Rm Old One", "-f", "."], repo)
            self.assertEqual(dep1_new.returncode, 0,
                             dep1_new.stdout + dep1_new.stderr)
            _, dep1_path = _extract_created_mdoc(dep1_new.stdout)

            dep2_new = _run_cli(["new", "-t", "Rm Old Two", "-f", "."], repo)
            self.assertEqual(dep2_new.returncode, 0,
                             dep2_new.stdout + dep2_new.stderr)
            _, dep2_path = _extract_created_mdoc(dep2_new.stdout)

            rc_add_1, out_add_1 = _run_cli_tty(
                ["dep", "add", src_path, "Rm Old One"], repo, b" \r")
            self.assertEqual(rc_add_1, 0, out_add_1)
            self.assertIn("added: 1", _compact_cli_output(out_add_1))

            rc_add_2, out_add_2 = _run_cli_tty(
                ["dep", "add", src_path, "Rm Old Two"], repo, b" \r")
            self.assertEqual(rc_add_2, 0, out_add_2)
            self.assertIn("added: 1", _compact_cli_output(out_add_2))

            _rewrite_title(dep1_path, "Rm New One")
            _rewrite_title(dep2_path, "Rm New Two")

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            self.assertIn("removed: 1", _compact_cli_output(out_rm))

            search_new = _run_cli(["search", "Rm New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertEqual(len(_search_result_lines(search_new.stdout)), 1)
            self.assertTrue(
                ("Rm New One" in search_new.stdout) ^ (
                    "Rm New Two" in search_new.stdout),
                search_new.stdout,
            )

    def test_incremental_index_with_edit_and_sync(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_incremental.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            alpha_new = _run_cli(["new", "-t", "Alpha", "-f", "."], repo)
            self.assertEqual(alpha_new.returncode, 0,
                             alpha_new.stdout + alpha_new.stderr)
            _, alpha_path = _extract_created_mdoc(alpha_new.stdout)

            beta_new = _run_cli(["new", "-t", "Beta", "-f", "."], repo)
            self.assertEqual(beta_new.returncode, 0,
                             beta_new.stdout + beta_new.stderr)
            _, beta_path = _extract_created_mdoc(beta_new.stdout)

            beta_file = Path(beta_path)
            beta_text = beta_file.read_text(encoding="utf-8")
            beta_file.write_text(
                beta_text.replace("@title: Beta", "@title: Gamma External"),
                encoding="utf-8",
            )

            before_sync_new = _run_cli(["search", "Gamma External"], repo)
            self.assertEqual(before_sync_new.returncode, 0)
            self.assertIn("No results for: Gamma External",
                          before_sync_new.stdout)

            before_sync_old = _run_cli(["search", "Beta"], repo)
            self.assertEqual(before_sync_old.returncode, 0)
            self.assertIn("Beta", before_sync_old.stdout)

            fake_bin = repo / "fake-bin"
            fake_bin.mkdir(parents=True, exist_ok=True)
            fake_editor = fake_bin / "fake-editor"
            fake_editor.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                "target=\"$1\"\n"
                "tmp=\"${target}.tmp\"\n"
                "sed 's/^@title: Alpha$/@title: Alpha Edited/' \"$target\" > \"$tmp\"\n"
                "mv \"$tmp\" \"$target\"\n",
                encoding="utf-8",
            )
            fake_editor.chmod(0o755)

            env_path = f"{fake_bin}{os.pathsep}{os.environ.get('PATH', '')}"
            edit_run = _run_cli(
                ["edit", alpha_path],
                repo,
                env_extra={"PATH": env_path, "EDITOR": "fake-editor"},
            )
            self.assertEqual(edit_run.returncode, 0,
                             edit_run.stdout + edit_run.stderr)
            self.assertIn("edited:", edit_run.stdout)

            alpha_edited = _run_cli(["search", "Alpha Edited"], repo)
            self.assertEqual(alpha_edited.returncode, 0,
                             alpha_edited.stdout + alpha_edited.stderr)
            self.assertIn("Alpha Edited", alpha_edited.stdout)

            gamma_still_missing = _run_cli(["search", "Gamma External"], repo)
            self.assertEqual(gamma_still_missing.returncode, 0)
            self.assertIn("No results for: Gamma External",
                          gamma_still_missing.stdout)

            sync_run = _run_cli(["sync"], repo)
            self.assertEqual(sync_run.returncode, 0,
                             sync_run.stdout + sync_run.stderr)
            self.assertRegex(sync_run.stdout, r"^synced: \d+$")

            gamma_after_sync = _run_cli(["search", "Gamma External"], repo)
            self.assertEqual(gamma_after_sync.returncode, 0,
                             gamma_after_sync.stdout + gamma_after_sync.stderr)
            self.assertIn("Gamma External", gamma_after_sync.stdout)

    def test_new_shows_warning_when_index_update_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_warn_new.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            db_path = repo / ".mdc" / "index.db"
            db_path.write_text("broken-db", encoding="utf-8")

            new_run = _run_cli(["new", "-t", "Warn Card", "-f", "."], repo)
            self.assertEqual(new_run.returncode, 0,
                             new_run.stdout + new_run.stderr)
            self.assertIn(
                "Warning: mdoc was created, but index refresh failed", new_run.stdout)
            self.assertIn("run `mdc sync`", new_run.stdout)
            _, created_path = _extract_created_mdoc(new_run.stdout)
            self.assertTrue(Path(created_path).is_file())

    def test_search_handles_corrupted_index_without_traceback(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_corrupt_index.") as tmp:
            repo = Path(tmp)
            self.assertEqual(_run_cli(["init"], repo).returncode, 0)

            db_path = repo / ".mdc" / "index.db"
            db_path.write_text("broken-db", encoding="utf-8")

            search_run = _run_cli(["search", "anything"], repo)
            combined = search_run.stdout + search_run.stderr
            self.assertEqual(search_run.returncode, 1, combined)
            self.assertIn("Error: failed to prepare search index", search_run.stdout)
            self.assertIn("Hint: run `mdc sync`", search_run.stdout)
            self.assertNotIn("Traceback", combined)


if __name__ == "__main__":
    unittest.main()
