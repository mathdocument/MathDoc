#!/usr/bin/env python3
"""Create a byte-preserving Mathlib graph bundle; never builds or downloads caches."""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.source.resolve()
    git = lambda *a: subprocess.check_output(["git", "-C", str(root), *a]).decode().strip()
    if git("status", "--porcelain", "--untracked-files=no"):
        parser.error("benchmark requires an unmodified tracked checkout")
    revision = git("rev-parse", "HEAD")
    origin = git("remote", "get-url", "origin")
    tracked = [p for p in git("ls-files", "-z").split("\0") if p]
    paths = sorted(p for p in tracked if p == "Mathlib.lean" or p.startswith("Mathlib/") and p.endswith(".lean"))
    assert paths and "Mathlib.lean" in paths
    modules = {p: p.removesuffix(".lean").replace("/", ".") for p in paths}
    identities = {m: str(uuid.uuid5(uuid.NAMESPACE_URL, origin + "#" + m)) for m in modules.values()}
    toolchain = (root / "lean-toolchain").read_text().strip()
    with tempfile.NamedTemporaryFile(mode="w", suffix=".txt") as files:
        files.write("".join(str(root / p) + "\n" for p in paths))
        files.flush()
        parsed = subprocess.check_output([
            "elan", "run", toolchain, "lean", "--run",
            str(Path(__file__).with_name("lean-imports.lean").resolve()), files.name,
        ], cwd=files.name.rsplit("/", 1)[0]).decode()
    imports = {str(Path(p).relative_to(root)): ms for p, ms in map(json.loads, parsed.splitlines())}
    nodes = []
    for path in paths:
        raw = (root / path).read_bytes()
        nodes.append({
            "fnode": identities[modules[path]], "title": modules[path], "module": modules[path],
            "depens": sorted({identities[m] for m in imports[path] if m in identities}),
            "blocks": [{"srctype": "lean", "content": raw.decode(), "metadata": {
                "repository": origin, "commit": revision, "path": path,
                "sha256": hashlib.sha256(raw).hexdigest(),
            }}],
        })
    reserved = {"lean-toolchain", "lakefile.lean", "lakefile.toml", "lake-manifest.json"}
    support, excluded = {}, []
    for path in tracked:
        if path in modules or path in reserved:
            continue
        if any(part.startswith(".") for part in Path(path).parts) or (root / path).is_symlink():
            excluded.append(path)
            continue
        try:
            support[path] = (root / path).read_bytes().decode()
        except UnicodeDecodeError:
            excluded.append(path)
    project = {
        "toolchain": toolchain, "lakefile": (root / "lakefile.lean").read_bytes().decode(),
        "lakefile_name": "lakefile.lean", "module_root": "Mathlib",
        "manifest": (root / "lake-manifest.json").read_bytes().decode(), "files": support,
    }
    bundle = {"nodes": nodes, "project": project}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(bundle, ensure_ascii=False, separators=(",", ":")) + "\n")
    external = collections.Counter(m for ms in imports.values() for m in ms if m not in identities)
    report = {"commit": revision, "repository": origin, "toolchain": toolchain,
              "nodes": len(nodes), "edges": sum(len(n["depens"]) for n in nodes),
              "lean_bytes": sum(len(n["blocks"][0]["content"].encode()) for n in nodes),
              "supporting_text_files": len(support), "excluded_support_files": excluded,
              "external_import_roots": sorted({m.split(".")[0] for m in external}),
              "official_cache_downloaded": False}
    args.output.with_suffix(".metadata.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "excluded_support_files"}, indent=2))


if __name__ == "__main__":
    main()
