#!/usr/bin/env python3
"""Test a source-free deployment with isolated names, ports and disposable volumes.

Build: docker build -t mathdoc-runtime:local .
Run:   python3 tests/docker-smoke.py [--image mathdoc-runtime:local]

LeanGround is not started here: the worker points at an unreachable address and must
stay up anyway. The deployment bind-mounts secrets/ from a temporary directory: with
colima or another VM that shares only the home directory, set TMPDIR under $HOME.
"""
import argparse
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="mathdoc-runtime:local")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    print("Checking that the image has no Lean toolchain...", flush=True)
    assert subprocess.run(["docker", "run", "--rm", "--pull", "never", "--network", "none",
                           "--entrypoint", "sh", args.image, "-c", "command -v lean lake elan"],
                          capture_output=True).returncode != 0
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    project_name = "mathdoc-smoke-" + secrets.token_hex(4)
    token = secrets.token_hex(24)
    # Never inherit a checkout's production passwords, port or Compose settings.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("MDC_", "MATHDOC_", "COMPOSE_"))}
    env.update(MDC_PORT=str(port), COMPOSE_PROJECT_NAME=project_name, MDC_TOKEN=token)
    with tempfile.TemporaryDirectory(prefix="mathdoc-docker-") as temporary:
        deployment = Path(temporary) / "deployment"
        subprocess.run([str(repo / "scripts/package-deployment"), str(deployment), args.image],
                       env=env, check=True)
        # A real external origin exercises reverse-proxy Host/Origin handling.
        origin = f"http://mathdoc.test:{port}"
        (deployment / "mathdoc.env").write_text(
            "LEANGROUND_SERVER_URL=http://127.0.0.1:9\nLEANGROUND_ACTOR=mathdoc-coordinator\n"
            f"MDC_PUBLIC_ORIGIN={origin}\n")
        (deployment / "secrets/actors.json").write_text(
            json.dumps({"smoke": {"token": token, "admin": True}}))
        (deployment / "secrets/leanground-token").write_text("unused-token\n")
        assert not (deployment / "Dockerfile").exists()
        assert not (deployment / ".env").exists()
        compose = ["docker", "compose", "--project-directory", str(deployment),
                   "-p", project_name, "-f", str(deployment / "compose.yaml")]

        def run(*arguments, content=None, timeout=180, error=None):
            result = subprocess.run([*compose, *arguments], env=env, input=content,
                                    text=True, capture_output=True, timeout=timeout,
                                    cwd=temporary)
            if error is not None:
                assert result.returncode != 0, arguments
                assert error in result.stdout + result.stderr, result.stdout + result.stderr
                return result.stderr
            if result.returncode:
                raise RuntimeError(f"{arguments}: {result.stdout}\n{result.stderr}")
            return result.stdout

        def cli(*arguments, **kwargs):
            return json.loads(run("exec", "-T", "-e", f"MDC_TOKEN={token}", "runtime", "mdc",
                                  *arguments, **kwargs))

        def http(path, data=None, headers=None, expected=200, auth=True):
            body = None if data is None else json.dumps(data).encode()
            merged = {"Content-Type": "application/json"}
            if auth:
                merged["Authorization"] = f"Bearer {token}"
            merged.update(headers or {})
            request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=body,
                                             headers=merged)
            try:
                response = urllib.request.urlopen(request, timeout=60)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                assert response.status == expected, (path, response.status)
                return response.read()

        def up(*extra):
            return run("up", "-d", "--no-build", "--pull", "never", "--wait",
                       "--wait-timeout", "180", *extra, timeout=240)

        config = json.loads(run("config", "--format", "json"))
        assert set(config["services"]) == {"database", "postgres", "migrate", "runtime", "worker"}
        assert {v["name"] for v in config["volumes"].values()} == {
            f"{project_name}-vol-{kind}" for kind in ("data", "postgres", "cache")}
        for service in ("migrate", "runtime", "worker"):
            assert "build" not in config["services"][service]
            assert config["services"][service]["image"] == args.image
        for service in ("database", "postgres", "migrate", "worker"):
            assert "ports" not in config["services"][service], service
        assert config["services"]["runtime"]["ports"][0]["host_ip"] == "127.0.0.1"
        assert config["networks"]["backend"]["internal"]
        for service in ("database", "postgres", "migrate"):
            assert set(config["services"][service]["networks"]) == {"backend"}, service
        # A fresh CI runner has only the runtime we just built. Fetch the pinned
        # databases explicitly before testing source-free, offline-image startup.
        run("pull", "--policy", "missing", "database", "postgres", timeout=600)
        passwords = {}
        try:
            print("Starting the exported deployment (no source, no .env, no build)...", flush=True)
            up()
            for name in ("database-password", "postgres-password"):
                path = deployment / "secrets" / name
                passwords[name] = path.read_text().strip()
                assert len(passwords[name]) == 64 and all(c in "0123456789abcdef" for c in passwords[name])
                assert path.stat().st_mode & 0o777 == 0o440
            assert int(run("exec", "-T", "runtime", "id", "-u")) == 10001
            assert b"<html" in http("/", auth=False)
            http("/api/me", auth=False, expected=401)
            assert json.loads(http("/api/me"))["actor"] == "smoke"
            http("/api/status", headers={"Host": f"mathdoc.test:{port}", "Origin": origin})
            http("/api/status", headers={"Host": "untrusted.example"}, expected=403)
            http("/api/status", headers={"Origin": "http://untrusted.example"}, expected=403)
            # Secret files: missing, empty and doubly set values fail with a clear message.
            migrate = ["node", "/app/coordinator/dist/main.js", "migrate"]
            for variables, error in (
                    (["-e", "MDC_DATABASE_PASSWORD_FILE=/missing"], "cannot read MDC_DATABASE_PASSWORD_FILE"),
                    (["-e", "MDC_DATABASE_PASSWORD_FILE=/dev/null"], "is empty"),
                    (["-e", "MDC_DATABASE_PASSWORD=conflicting"], "set only one of")):
                run("exec", "-T", *variables, "runtime", *migrate, error=error)
            assert cli("status")["server"]["on_demand"] is True
            cli("init", "smoke")
            cli("-p", "smoke/main", "branch", "new", "review")
            node = cli("-p", "smoke/main", "new", "-t", "Container proof")
            wrapped = subprocess.run([str(deployment / "mdc"), "-p", "smoke/main", "edit",
                                      node["fnode"]], cwd=temporary, env=env,
                                     input="theorem container_proof : 1 + 1 = 2 := rfl\n",
                                     text=True, capture_output=True, check=True, timeout=60)
            assert json.loads(wrapped.stdout)["fnode"] == node["fnode"]
            preview = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/latex/preview",
                                      {"source": r"\section{Runtime}A formula: $1+1=2$."}))
            assert preview["html"] and not preview["diagnostics"], preview
            view = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/view"))
            assert view["node"]["formalization"]["lean_certification"] == {"status": "not_submitted"}
            for action in ("restart", "recreate", "down-up"):
                print(f"Checking {action}, credentials and persistent state...", flush=True)
                if action == "restart":
                    run("restart")
                elif action == "down-up":
                    run("down")
                up(*(["--force-recreate"] if action == "recreate" else []))
                for name, value in passwords.items():
                    assert (deployment / "secrets" / name).read_text().strip() == value
                assert {"smoke/main", "smoke/review"} <= set(cli("status")["projects"])
                assert cli("-p", "smoke/main", "show", node["fnode"])["title"] == "Container proof"
                states = [json.loads(line) for line in
                          run("ps", "--format", "json", "worker").splitlines() if line.strip()]
                assert states and all(s.get("State") == "running" for s in states), states
            logs = run("logs", "--no-color")
            for service in ("runtime", "worker", "database", "postgres"):
                inspect = subprocess.run(["docker", "inspect", f"{project_name}-{service}"],
                                         text=True, capture_output=True, check=True).stdout
                for value in (*passwords.values(), token):
                    assert value not in inspect, service
                    assert value not in logs
            print("Checking lost-secret recovery without resetting the databases...", flush=True)
            for service, name in (("database", "database-password"), ("postgres", "postgres-password")):
                path = deployment / "secrets" / name
                saved = path.with_suffix(".saved")
                run("stop")
                path.rename(saved)
                failed = subprocess.run([*compose, "up", "-d", "--no-build", "--pull", "never",
                                         "--wait", "--wait-timeout", "15", service],
                                        env=env, cwd=temporary, capture_output=True, text=True, timeout=45)
                assert failed.returncode != 0
                assert not path.exists()
                assert "restore the original password file" in run("logs", service)
                run("stop", service)
                path.write_text("\n")
                failed = subprocess.run([*compose, "up", "-d", "--no-build", "--pull", "never",
                                         "--wait", "--wait-timeout", "15", service],
                                        env=env, cwd=temporary, capture_output=True, text=True, timeout=45)
                assert failed.returncode != 0
                assert "is empty or invalid" in run("logs", service)
                run("stop", service)
                path.unlink()
                saved.rename(path)
                # Restoring a file need not retain its original container group.
                path.chmod(0o600)
                os.chown(path, -1, os.getgid())
            up()
            for name in passwords:
                assert (deployment / "secrets" / name).stat().st_mode & 0o777 == 0o440
            assert cli("-p", "smoke/main", "show", node["fnode"])["title"] == "Container proof"
            print("Docker smoke passed: source-free startup, credentials, HTTP origins, tokens, CLI, "
                  "LaTeX, worker, restart, recreation, down/up and lost-secret recovery.", flush=True)
        except BaseException:
            subprocess.run([*compose, "logs", "--tail", "80"], env=env, timeout=30)
            raise
        finally:
            run("down", "--volumes", "--remove-orphans", timeout=120)


if __name__ == "__main__":
    main()
