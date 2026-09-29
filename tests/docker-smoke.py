#!/usr/bin/env python3
"""Exercise the server image with isolated volumes; never touch user projects.

Build first: docker compose -f compose.server.yaml build
Run: python3 tests/docker-smoke.py

LeanGround is not started here: the worker is pointed at an unreachable address and must
stay up anyway. Set LEANGROUND_SERVER_URL/LEANGROUND_FACT_TOKEN to use a real one.
"""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import urllib.error
import urllib.request


def main():
    repo = Path(__file__).resolve().parents[1]
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    project_name = "mdc-smoke-" + secrets.token_hex(4)
    token = secrets.token_hex(24)
    env = dict(os.environ, MDC_TERMINUS_PASSWORD=secrets.token_hex(32),
               MDC_POSTGRES_PASSWORD=secrets.token_hex(32), MDC_PORT=str(port),
               MDC_ACTORS=json.dumps({"smoke": {"token": token, "admin": True}}),
               LEANGROUND_SERVER_URL=os.environ.get("LEANGROUND_SERVER_URL", "http://127.0.0.1:9"),
               LEANGROUND_FACT_TOKEN=os.environ.get("LEANGROUND_FACT_TOKEN", "unused"),
               LEANGROUND_ACTOR=os.environ.get("LEANGROUND_ACTOR", "mathdoc-coordinator"),
               MDC_TOKEN=token, COMPOSE_PROJECT_NAME=project_name)
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
            return json.loads(run("exec", "-T", "-e", f"MDC_TOKEN={token}", "mdc", "sh", "-c",
                                  'MDC_URL="http://127.0.0.1:$MDC_PORT" exec mdc "$@"', "mdc",
                                  *args, **kwargs))

        def http(path, data=None, auth=True):
            body = None if data is None else json.dumps(data).encode()
            headers = {"Content-Type": "application/json"}
            if auth:
                headers["Authorization"] = f"Bearer {token}"
            request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=body,
                                             headers=headers)
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.read()

        try:
            # The documented command: all services, including the one-shot migrate.
            run("up", "-d", "--no-build", "--wait", "--wait-timeout", "180", timeout=240)
            assert int(run("exec", "-T", "mdc", "id", "-u")) != 0
            assert b"<html" in http("/", auth=False)
            try:
                http("/api/me", auth=False)
                raise AssertionError("/api/me without a token must be refused")
            except urllib.error.HTTPError as e:
                assert e.code == 401, e.code
            assert json.loads(http("/api/me"))["actor"] == "smoke"
            # No Lean toolchain in the image (plan §8).
            assert subprocess.run([*compose, "exec", "-T", "mdc", "sh", "-c",
                                   "command -v lean lake elan"], env=env,
                                  capture_output=True).returncode != 0
            assert cli("status")["server"]["on_demand"] is True
            cli("init", "smoke")
            cli("-p", "smoke/main", "branch", "new", "review")
            node = cli("-p", "smoke/main", "new", "-t", "Container proof")
            cli("-p", "smoke/main", "edit", node["fnode"],
                content="theorem container_proof : 1 + 1 = 2 := rfl\n")
            wrapped = subprocess.run([str(repo / "scripts/mdc-docker"), "-p", "smoke/main",
                                      "show", node["fnode"]], cwd=temporary, env=env,
                                     text=True, capture_output=True, check=True, timeout=60)
            assert json.loads(wrapped.stdout)["fnode"] == node["fnode"]
            preview = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/latex/preview",
                                      {"source": r"\section{Runtime}A formula: $1+1=2$."}))
            assert preview["html"] and not preview["diagnostics"], preview
            view = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/view"))
            assert view["node"]["formalization"]["lean_certification"] == {"status": "not_submitted"}
            for action in ("restart", "recreate"):
                if action == "restart":
                    run("restart", "mdc", "worker")
                run("up", "-d", "--no-build", "--wait", "--wait-timeout", "180",
                    *(["--force-recreate"] if action == "recreate" else []), "mdc", "worker",
                    timeout=240)
                projects = cli("status")["projects"]
                assert {"smoke/main", "smoke/review"} <= set(projects), projects
                assert cli("-p", "smoke/main", "show", node["fnode"])["title"] == "Container proof"
                # One JSON object per line (Compose v2.21+).
                states = [json.loads(line) for line in
                          run("ps", "--format", "json", "worker").splitlines() if line.strip()]
                assert states and all(s.get("State") == "running" for s in states), states
            print("Docker smoke passed: HTTP, tokens, CLI, LaTeX, worker, restart and recreation.")
        except BaseException:
            subprocess.run([*compose, "logs", "--tail", "80"], env=env, timeout=30)
            raise
        finally:
            run("down", "--volumes", "--remove-orphans", timeout=120)


if __name__ == "__main__":
    main()
