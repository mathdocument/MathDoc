#!/usr/bin/env python3
"""Exercise the server image with isolated volumes; never touch user projects.

Build first: docker compose -f compose.server.yaml build
Run: python3 tests/docker-smoke.py
"""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import urllib.request


def main():
    repo = Path(__file__).resolve().parents[1]
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    project_name = "mdc-smoke-" + secrets.token_hex(4)
    env = dict(os.environ, MDC_TERMINUS_PASSWORD=secrets.token_hex(32), MDC_PORT=str(port),
               COMPOSE_PROJECT_NAME=project_name)
    with tempfile.TemporaryDirectory(prefix="mdc-docker-") as temporary:
        # An explicit empty env file prevents a checkout's production .env being used.
        env_file = Path(temporary) / "empty.env"
        env_file.touch()
        compose = ["docker", "compose", "--env-file", str(env_file), "-p",
                   project_name, "-f", str(repo / "compose.server.yaml")]

        def run(*args, content=None, timeout=180):
            result = subprocess.run([*compose, *args], env=env, input=content,
                                    text=True, capture_output=True, timeout=timeout)
            if result.returncode:
                raise RuntimeError(f"{args}: {result.stdout}\n{result.stderr}")
            return result.stdout

        def cli(*args, **kwargs):
            return json.loads(run("exec", "-T", "mdc", "mdc", *args, **kwargs))

        def http(path, data=None):
            body = None if data is None else json.dumps(data).encode()
            request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=body,
                                             headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.read()

        try:
            run("up", "-d", "--no-build", "--wait", "--wait-timeout", "180", timeout=240)
            assert int(run("exec", "-T", "mdc", "id", "-u")) != 0
            assert b"<html" in http("/")
            assert cli("status")["server"]["port"] == port
            cli("init", "smoke")
            cli("start", "smoke/main")
            cli("branch", "new", "paused", "-p", "smoke/main")
            cli("start", "smoke/paused")
            cli("stop", "smoke/paused")
            node = cli("new", "-p", "smoke/main", "-t", "Container proof")
            cli("edit", "-p", "smoke/main", node["fnode"],
                content="theorem container_proof : 1 + 1 = 2 := rfl\n")
            wrapped = subprocess.run([str(repo / "scripts/mdc-docker"), "show", "-p",
                                      "smoke/main", node["fnode"]], cwd=temporary, env=env,
                                     text=True, capture_output=True, check=True, timeout=60)
            assert json.loads(wrapped.stdout)["fnode"] == node["fnode"]
            project = cli("project", "show", "-p", "smoke/main")
            toolchain = project["project"]["toolchain"]
            print(f"Installing {toolchain} in the disposable toolchain volume…", flush=True)
            run("exec", "-T", "mdc", "elan", "toolchain", "install", toolchain, timeout=1200)
            checked = cli("lean", "check", "--build", "-p", "smoke/main", node["fnode"], timeout=360)
            assert checked["certified"] and checked["built"], checked
            preview = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/latex/preview",
                                      {"source": r"\section{Runtime}A formula: $1+1=2$."}))
            assert preview["html"] and not preview["diagnostics"], preview
            for action in ("restart", "recreate"):
                if action == "restart":
                    run("restart", "mdc")
                run("up", "-d", "--no-build", "--wait", "--wait-timeout", "180",
                    *(["--force-recreate"] if action == "recreate" else []), timeout=240)
                states = cli("status")["projects"]
                assert states["smoke/main"]["running"]
                assert not states["smoke/paused"]["running"]
                assert cli("show", "-p", "smoke/main", node["fnode"])["title"] == "Container proof"
                checked = cli("lean", "check", "-p", "smoke/main", node["fnode"])
                assert checked["certified"] and checked["cache_hit"], checked
            print("Docker smoke passed: HTTP, CLI, Lean, LaTeX, restart, recreation and cache reuse.")
        except BaseException:
            subprocess.run([*compose, "logs", "--tail", "80"], env=env, timeout=30)
            raise
        finally:
            run("down", "--volumes", "--remove-orphans", timeout=120)


if __name__ == "__main__":
    main()
