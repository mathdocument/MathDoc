#!/usr/bin/env python3
"""Measure an existing service without compiling Lean. --writes needs a disposable branch."""
import argparse
import collections
import concurrent.futures
import json
import math
import platform
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

# Local service measurements must not include macOS proxy auto-discovery per request.
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def request(url, path, method="GET", body=None, revision=None, status=200):
    headers = {"Content-Type": "application/json"}
    if revision is not None:
        headers["If-Match"] = json.dumps(revision)
    req = urllib.request.Request(
        url.rstrip("/") + "/api" + path, method=method, headers=headers,
        data=None if body is None else json.dumps(body).encode(),
    )
    started = time.perf_counter()
    try:
        response = HTTP.open(req, timeout=180)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read()
        value = json.loads(raw)
        assert response.status == status, (path, response.status, value)
    return value, (time.perf_counter() - started) * 1000, len(raw)


def statistics_ms(samples):
    return {
        "samples_ms": [round(s, 3) for s in samples],
        "median_ms": round(statistics.median(samples), 3),
        "p95_ms": round(sorted(samples)[math.ceil(len(samples) * .95) - 1], 3),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--cli", help="Installed mdc executable for fresh-process measurements")
    parser.add_argument("--proj", help="DATABASE/BRANCH served at --url; required with --cli")
    parser.add_argument("--query", default="equation", help="Representative title search term")
    parser.add_argument("--writes", action="store_true", help="Adds nodes: use ONLY on a disposable database branch")
    args = parser.parse_args()
    if args.cli and not args.proj:
        parser.error("--cli requires --proj DATABASE/BRANCH")
    if args.samples < 1:
        parser.error("samples must be positive")
    report = {"platform": platform.platform(), "url": args.url, "query": args.query, "reads": {}, "cli": {}, "writes": {}}
    graph, _, _ = request(args.url, "/graph/full")
    nodes, edges = graph["nodes"], graph["edges"]
    assert nodes, "benchmark needs a populated graph"
    outdegree = collections.Counter(a for a, _ in edges)
    indegree = collections.Counter(b for _, b in edges)
    highout = max(range(len(nodes)), key=lambda i: outdegree[i])
    highin = max(range(len(nodes)), key=lambda i: indegree[i])
    deep = max(range(len(nodes)), key=lambda i: nodes[i]["depth"])
    titles = collections.Counter(n["title"] for n in nodes)
    unique = next(n for n in nodes if titles[n["title"]] == 1)
    report["graph"] = {"nodes": len(nodes), "edges": len(edges)}
    report["representatives"] = {
        label: {**nodes[i], "out_degree": outdegree[i], "in_degree": indegree[i]}
        for label, i in [("high_out_degree", highout), ("high_in_degree", highin), ("deep", deep)]
    }
    source, target, deepest = [nodes[i]["fnode"] for i in (highout, highin, deep)]
    paths = {
        "graph_check": "/graph/check", "graph_roots": "/graph/roots", "graph_full": "/graph/full",
        "search_common": "/search?" + urllib.parse.urlencode({"q": args.query, "n": 20}),
        "search_missing": "/search?q=__mdc_no_matching_title__&n=20",
        "resolve_uuid": "/resolve?" + urllib.parse.urlencode({"ref": source}),
        "resolve_name": "/resolve?" + urllib.parse.urlencode({"ref": unique["title"]}),
        "view_high_out_degree": f"/node/{source}/view", "view_high_in_degree": f"/node/{target}/view",
        "deps_direct": f"/node/{source}/dep?mode=show&depth=1",
        "deps_transitive": f"/node/{deepest}/dep?mode=show&depth=-1",
        "refs_direct": f"/node/{target}/dep?mode=refs&depth=1",
        "refs_transitive": f"/node/{target}/dep?mode=refs&depth=-1",
        "leaves": f"/node/{deepest}/dep?mode=leaf&depth=-1",
        "candidates": f"/node/{source}/dep/candidates?q=&n=50",
        "ior": f"/node/{source}/metric/ior", "history": "/history",
    }
    for label, path in paths.items():
        request(args.url, path)  # one untimed warmup
        samples = [request(args.url, path) for _ in range(args.samples)]
        value = samples[-1][0]
        if label == "graph_check":
            assert value == report["graph"]
        report["reads"][label] = {
            **statistics_ms([sample[1] for sample in samples]),
            "response_bytes": samples[-1][2],
            "returned_items": len(value) if isinstance(value, list) else None,
        }
        print(label, report["reads"][label]["median_ms"], "ms", file=sys.stderr, flush=True)
    mixed = [paths[k] for k in ("graph_check", "graph_roots", "search_common", "deps_transitive", "refs_transitive")] * 8
    started = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        samples = list(pool.map(lambda path: request(args.url, path), mixed))
    report["concurrent_reads"] = {**statistics_ms([s[1] for s in samples]), "clients": 8, "requests": len(mixed), "elapsed_ms": round((time.perf_counter() - started) * 1000, 3)}
    if args.cli:
        commands = {
            "graph_check": ["graph", "check"], "graph_roots": ["graph", "roots"], "graph_full": ["graph", "full"],
            "search": ["search", args.query, "-n", "20"], "show": ["show", source],
            "deps_transitive": ["dep", "show", deepest, "--depth", "-1"],
            "refs_transitive": ["dep", "refs", target, "--depth", "-1"],
            "leaves": ["dep", "leaf", deepest], "ior": ["metric", "ior", source],
        }
        for label, command in commands.items():
            samples = []
            for _ in range(args.samples):
                started = time.perf_counter()
                result = subprocess.run([args.cli, *command, "--proj", args.proj], capture_output=True, check=True, timeout=180)
                json.loads(result.stdout)
                samples.append((time.perf_counter() - started) * 1000)
            report["cli"][label] = statistics_ms(samples)
            print("cli", label, report["cli"][label]["median_ms"], "ms", file=sys.stderr, flush=True)
    if args.writes:
        prefix = "Benchmark " + uuid.uuid4().hex
        a, _, _ = request(args.url, "/node/new", "POST", {"title": prefix + " A"})
        b, _, _ = request(args.url, "/node/new", "POST", {"title": prefix + " B"})
        timings = collections.defaultdict(list)
        for i in range(args.samples):
            _, ms, _ = request(args.url, "/node/new", "POST", {"title": f"{prefix} {i}"})
            timings["create_node"].append(ms)
            previous = a["revision"]
            a, ms, _ = request(args.url, f"/node/{a['fnode']}/title", "PUT", {"title": f"{prefix} A {i}"}, previous)
            timings["rename"].append(ms)
            _, ms, _ = request(args.url, f"/node/{a['fnode']}/title", "PUT", {"title": "Stale write"}, previous, status=412)
            timings["reject_stale_write"].append(ms)
            a, ms, _ = request(args.url, f"/node/{a['fnode']}/block/text", "PUT", {"content": f"Benchmark {i}"}, a["revision"])
            timings["save_text"].append(ms)
            a, ms, _ = request(args.url, f"/node/{a['fnode']}/dep/add", "POST", {"dep_fnode": b["fnode"]}, a["revision"])
            timings["add_edge"].append(ms)
            _, ms, _ = request(args.url, f"/node/{b['fnode']}/dep/add", "POST", {"dep_fnode": a["fnode"]}, b["revision"], status=422)
            timings["reject_cycle"].append(ms)
            a, ms, _ = request(args.url, f"/node/{a['fnode']}/dep/rm", "POST", {"dep_fnodes": [b["fnode"]]}, a["revision"])
            timings["remove_edge"].append(ms)
        after, _, _ = request(args.url, "/graph/check")
        assert after == {"nodes": len(nodes) + args.samples + 2, "edges": len(edges)}
        report["writes"] = {label: statistics_ms(values) for label, values in timings.items()}
    with open(args.output, "w") as file:
        json.dump(report, file, indent=2, ensure_ascii=False)
        file.write("\n")


if __name__ == "__main__":
    main()
