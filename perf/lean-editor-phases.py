#!/usr/bin/env python3
"""Time Lean's native file worker against an external, populated Lake workspace.

For Lean 4.33.1; dependencies are not rebuilt. Each sample uses a fresh worker,
existing artifacts, and a 4 GiB Lean memory limit. The internal header-setup
notification and profiler import-end marker delimit the phases; body time
includes waiting for diagnostics. Wall-clock times include profiling overhead.
Runtime data and results must stay outside the source tree.
"""
import argparse
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import time


async def measure(root, module, env):
    source = root / (module.replace(".", "/") + ".lean")
    uri = source.as_uri()
    process = await asyncio.create_subprocess_exec(
        "elan", "run", (root / "lean-toolchain").read_text().strip(), "lean",
        "--memory=4096", "-Dprofiler=true", "-Dprofiler.threshold=0", "--worker", uri,
        cwd=root, env=env, stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    started = time.perf_counter()
    stamps = {}
    logs = []

    async def read_stderr():
        while line := await process.stderr.readline():
            text = line.decode()
            if text.startswith("import took "):
                stamps["imported"] = (time.perf_counter() - started) * 1000
            if " took " not in text:
                logs.append(text)
    stderr = asyncio.create_task(read_stderr())

    async def send(message):
        data = json.dumps({"jsonrpc": "2.0", **message}).encode()
        process.stdin.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
        await process.stdin.drain()

    async def collect():
        errors = []
        await send({"id": 0, "method": "initialize", "params": {
            "processId": None, "rootUri": root.as_uri(), "capabilities": {},
            "initializationOptions": {"hasWidgets": True}}})
        await send({"method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "lean4", "version": 1,
                             "text": source.read_text()}, "dependencyBuildMode": "never"}})
        await send({"id": 1, "method": "textDocument/waitForDiagnostics",
                    "params": {"uri": uri, "version": 1}})
        while True:
            header = await process.stdout.readuntil(b"\r\n\r\n")
            size = int(next(line.split(b":")[1] for line in header.splitlines()
                            if line.lower().startswith(b"content-length:")))
            message = json.loads(await process.stdout.readexactly(size))
            elapsed = (time.perf_counter() - started) * 1000
            method = message.get("method")
            if method == "$/lean/ileanHeaderSetupInfo":
                stamps[method] = elapsed
            if method == "textDocument/publishDiagnostics":
                errors.extend(d for d in message["params"]["diagnostics"] if d.get("severity") == 1)
            if "id" in message and method:
                await send({"id": message["id"], "result": None})
            if message.get("id") == 1 and not method:
                assert "error" not in message, message
                stamps["ready"] = elapsed
            if errors:
                raise RuntimeError(errors)
            if "ready" in stamps:
                break
        # Drain the profiler marker, which travels on stderr rather than LSP stdout.
        while "imported" not in stamps:
            await asyncio.sleep(0.001)
        setup, imported, ready = (stamps[key] for key in
                                  ("$/lean/ileanHeaderSetupInfo", "imported", "ready"))
        # A tiny body can finish before the asynchronous stderr marker arrives.
        # Retain the skew rather than reporting a negative duration as body work.
        return {"module": module, "setup_ms": setup,
                "imports_ms": min(imported, ready) - setup, "body_ms": max(0, ready - imported),
                "notification_skew_ms": max(0, imported - ready), "total_ms": ready}

    try:
        return await asyncio.wait_for(collect(), 120)
    finally:
        if process.returncode is None:
            process.kill()
        await process.wait()
        await stderr
        if logs:
            print("".join(logs), file=sys.stderr)


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("modules", nargs="+")
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()
    root = args.root.resolve()
    env = {k: v for k, v in os.environ.items() if not k.startswith(("MDC_", "MATHDOC_"))}
    env.update(LAKE_ARTIFACT_CACHE="true", LAKE_RESTORE_ARTIFACTS="false",
               LAKE_CACHE_DIR=str(root / ".lake/cache"))
    # Match the environment inherited by file workers from `lake serve`.
    output = subprocess.check_output(["elan", "run", (root / "lean-toolchain").read_text().strip(),
                                      "lake", "env", "env", "-0"], cwd=root, env=env)
    env = dict(item.decode().split("=", 1) for item in output.split(b"\0") if item)
    for module in args.modules:
        for sample in range(args.runs):
            result = await measure(root, module, env)
            print(json.dumps({"sample": sample, **result}), flush=True)


if __name__ == "__main__":
    asyncio.run(main())
