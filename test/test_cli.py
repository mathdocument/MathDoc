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


def _run_cli_tty(args: list[str], cwd: Path, keys: bytes) -> tuple[int, str]:
    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        _cli_base_cmd() + args,
        cwd=str(cwd),
        env=_cli_env(),
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        close_fds=True,
    )
    os.close(slave_fd)

    output = bytearray()
    sent = False
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
                if (b"Select deps:" in output) and not sent:
                    os.write(master_fd, keys)
                    sent = True

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

        if proc.poll() is None and not sent:
            os.write(master_fd, keys)
            sent = True
            proc.wait(timeout=5)
    finally:
        try:
            os.close(master_fd)
        except OSError:
            pass
        if proc.poll() is None:
            proc.kill()
            proc.wait()

    return proc.returncode, output.decode("utf-8", errors="replace")


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

            new_1 = _run_cli(["new", "-t", "Linear Algebra", "-f", "."], repo)
            self.assertEqual(new_1.returncode, 0, new_1.stdout + new_1.stderr)
            fnode_1, path_1 = _extract_created_mdoc(new_1.stdout)
            self.assertTrue(Path(path_1).is_file())

            new_2 = _run_cli(["new", "-t", "Matrix Rank", "-f", "notes"], repo)
            self.assertEqual(new_2.returncode, 0, new_2.stdout + new_2.stderr)
            _, path_2 = _extract_created_mdoc(new_2.stdout)
            self.assertTrue(Path(path_2).is_file())

            outside = repo.parent / "outside"
            outside.mkdir(parents=True, exist_ok=True)
            new_outside = _run_cli(
                ["new", "-t", "Bad", "-f", str(outside)], repo)
            self.assertEqual(new_outside.returncode, 1)
            self.assertIn("target path must be under mdoc root",
                          new_outside.stdout)

            search_title = _run_cli(["search", "matrix"], repo)
            self.assertEqual(search_title.returncode, 0,
                             search_title.stdout + search_title.stderr)
            self.assertIn("results: 1", search_title.stdout)
            self.assertRegex(
                search_title.stdout,
                r"(?m)^- [0-9a-f]{8}\s+Matrix Rank \(.+\.mdoc\)$",
            )

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
            self.assertIn("blocks: 2", eval_run.stdout)
            self.assertIn("[1] natl: ok", eval_run.stdout)
            self.assertIn("hello from natl", eval_run.stdout)
            self.assertIn("[2] py: ok", eval_run.stdout)
            self.assertIn("hello from py", eval_run.stdout)
            self.assertIn("failed: 0", eval_run.stdout)

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
            self.assertIn("[1] py: failed (1)", eval_run.stdout)
            self.assertIn("boom", eval_run.stdout)
            self.assertIn("failed: 1", eval_run.stdout)

    def test_eval_accepts_depth_and_reverse_flags(self) -> None:
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
            self.assertIn("added: 1", out_add_root)

            rc_add_dep1, out_add_dep1 = _run_cli_tty(
                ["dep", "add", dep1_path, "Dep Two"], repo, b" \r"
            )
            self.assertEqual(rc_add_dep1, 0, out_add_dep1)
            self.assertIn("added: 1", out_add_dep1)

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

            eval_reversed = _run_cli(
                ["eval", "--depth", "-1", "--reverse", root_path], repo)
            self.assertEqual(eval_reversed.returncode, 0,
                             eval_reversed.stdout + eval_reversed.stderr)
            root_pos = eval_reversed.stdout.find("root-body")
            dep1_pos = eval_reversed.stdout.find("dep1-body")
            dep2_pos = eval_reversed.stdout.find("dep2-body")
            self.assertGreaterEqual(root_pos, 0, eval_reversed.stdout)
            self.assertGreaterEqual(dep1_pos, 0, eval_reversed.stdout)
            self.assertGreaterEqual(dep2_pos, 0, eval_reversed.stdout)
            self.assertLess(root_pos, dep1_pos, eval_reversed.stdout)
            self.assertLess(dep1_pos, dep2_pos, eval_reversed.stdout)

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
            self.assertIn(f"snippet_{src_fnode}.tex", eval_run.stdout)
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
            self.assertIn("dependencies: 0", show_0.stdout)

            rc_add, out_add = _run_cli_tty(
                ["dep", "add", src_path, "matrix"], repo, b" \r")
            self.assertEqual(rc_add, 0, out_add)
            self.assertIn("added: 1", out_add)
            self.assertRegex(
                out_add,
                rf"\+ {dep_fnode[:8]}\s+Matrix Rank \({re.escape(dep_rel)}\)",
            )
            self.assertRegex(
                out_add,
                rf"source:\s+{src_fnode[:8]}\s+Source Card",
            )

            show_1 = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_1.returncode, 0,
                             show_1.stdout + show_1.stderr)
            self.assertIn("dependencies: 1", show_1.stdout)
            self.assertRegex(
                show_1.stdout,
                rf"- {dep_fnode[:8]}\s+Matrix Rank \({re.escape(dep_rel)}\)",
            )

            rc_add_dup, out_add_dup = _run_cli_tty(
                ["dep", "add", src_path, "matrix"], repo, b" \r")
            self.assertEqual(rc_add_dup, 0, out_add_dup)
            self.assertIn("added: 0", out_add_dup)
            self.assertIn("skipped existing: 1", out_add_dup)

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            self.assertIn("removed: 1", out_rm)
            self.assertRegex(
                out_rm,
                rf"- {dep_fnode[:8]}\s+Matrix Rank \({re.escape(dep_rel)}\)",
            )

            show_2 = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_2.returncode, 0,
                             show_2.stdout + show_2.stderr)
            self.assertIn("dependencies: 0", show_2.stdout)

    def test_nested_repo_direction_parent_to_child_only(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_cli_nested.") as tmp:
            parent = Path(tmp) / "parent"
            child = parent / "child"
            parent.mkdir(parents=True)
            child.mkdir(parents=True)

            self.assertEqual(_run_cli(["init"], parent).returncode, 0)
            self.assertEqual(
                _run_cli(["new", "-t", "Parent Only", "-f", "."], parent).returncode, 0)

            self.assertEqual(_run_cli(["init"], child).returncode, 0)
            self.assertEqual(
                _run_cli(["new", "-t", "Child Only", "-f", "."], child).returncode, 0)

            parent_search_child = _run_cli(["search", "Child Only"], parent)
            self.assertEqual(
                parent_search_child.returncode, 0, parent_search_child.stdout +
                parent_search_child.stderr
            )
            self.assertIn("Child Only", parent_search_child.stdout)

            child_search_parent = _run_cli(["search", "Parent Only"], child)
            self.assertEqual(
                child_search_parent.returncode, 0, child_search_parent.stdout +
                child_search_parent.stderr
            )
            self.assertIn("No results for: Parent Only",
                          child_search_parent.stdout)

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
            self.assertIn("added: 1", out_add)

            _rewrite_title(dep_path, "Show Dep New")

            show_run = _run_cli(["dep", "show", src_path], repo)
            self.assertEqual(show_run.returncode, 0,
                             show_run.stdout + show_run.stderr)
            self.assertIn("Show Dep New", show_run.stdout)

            search_new = _run_cli(["search", "Show Dep New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertIn("Show Dep New", search_new.stdout)

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
            self.assertIn("added: 1", out_add)

            search_new = _run_cli(["search", "Add New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertIn("results: 1", search_new.stdout)
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
            self.assertIn("added: 1", out_add_1)

            rc_add_2, out_add_2 = _run_cli_tty(
                ["dep", "add", src_path, "Rm Old Two"], repo, b" \r")
            self.assertEqual(rc_add_2, 0, out_add_2)
            self.assertIn("added: 1", out_add_2)

            _rewrite_title(dep1_path, "Rm New One")
            _rewrite_title(dep2_path, "Rm New Two")

            rc_rm, out_rm = _run_cli_tty(["dep", "rm", src_path], repo, b" \r")
            self.assertEqual(rc_rm, 0, out_rm)
            self.assertIn("removed: 1", out_rm)

            search_new = _run_cli(["search", "Rm New"], repo)
            self.assertEqual(search_new.returncode, 0,
                             search_new.stdout + search_new.stderr)
            self.assertIn("results: 1", search_new.stdout)
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


if __name__ == "__main__":
    unittest.main()
