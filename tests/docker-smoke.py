#!/usr/bin/env python3
"""Test a source-free deployment with isolated names, ports and disposable volumes.

Build: docker build -t mathdoc-runtime:local .
Run:   python3 tests/docker-smoke.py [--image mathdoc-runtime:local]
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
    print("Checking the bundled toolchain offline with a fresh tools volume…", flush=True)
    subprocess.run([
        "docker", "run", "--rm", "--pull", "never", "--network", "none", "--mount",
        "type=volume,destination=/var/lib/mdc/tools", "--entrypoint", "sh", args.image,
        "-ec", "mathdoc-entrypoint --help >/dev/null; lean --version; lake --version; "
        "test -L \"$ELAN_HOME/toolchains/leanprover--lean4---v4.33.1\"",
    ], check=True)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    project_name = "mathdoc-smoke-" + secrets.token_hex(4)
    # Never inherit a checkout's production password, port or Compose settings.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("MDC_", "MATHDOC_", "COMPOSE_"))}
    env.update(MDC_PORT=str(port), COMPOSE_PROJECT_NAME=project_name)
    with tempfile.TemporaryDirectory(prefix="mathdoc-docker-") as temporary:
        deployment = Path(temporary) / "deployment"
        subprocess.run([str(repo / "scripts/package-deployment"), str(deployment), args.image],
                       env=env, check=True)
        # A real external origin exercises reverse-proxy Host/Origin handling.
        origin = f"http://mathdoc.test:{port}"
        (deployment / "config.toml").write_text(f'public_origin = "{origin}"\n')
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
                assert error in result.stdout + result.stderr, result.stderr
                return result.stderr
            if result.returncode:
                raise RuntimeError(f"{arguments}: {result.stdout}\n{result.stderr}")
            return result.stdout

        def cli(*arguments, **kwargs):
            return json.loads(run("exec", "-T", "runtime", "mdc", *arguments, **kwargs))

        def http(path, data=None, headers=None, expected=200):
            body = None if data is None else json.dumps(data).encode()
            request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=body,
                                             headers={"Content-Type": "application/json",
                                                      **(headers or {})})
            try:
                response = urllib.request.urlopen(request, timeout=60)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                assert response.status == expected, response.status
                return response.read()

        def up(*extra):
            return run("up", "-d", "--no-build", "--pull", "never", "--wait",
                       "--wait-timeout", "180", *extra, timeout=240)

        config = json.loads(run("config", "--format", "json"))
        assert set(config["services"]) == {"database", "runtime"}
        assert {v["name"] for v in config["volumes"].values()} == {
            f"{project_name}-vol-{kind}" for kind in ("data", "cache", "tools")}
        assert "build" not in config["services"]["runtime"]
        assert "ports" not in config["services"]["database"]
        assert config["services"]["runtime"]["ports"][0]["host_ip"] == "127.0.0.1"
        assert config["networks"]["backend"]["internal"]
        assert config["services"]["database"]["networks"] == {"backend": None}
        # A fresh CI runner has only the runtime we just built. Fetch the pinned
        # database explicitly before testing source-free, offline-image startup.
        run("pull", "--policy", "missing", "database", timeout=600)
        password_file = deployment / "secrets/database-password"
        saved_password = deployment / "secrets/database-password.saved"
        try:
            print("Starting the exported deployment (no source, no .env, no build)…", flush=True)
            up()
            password = password_file.read_text().strip()
            assert len(password) == 64 and all(c in "0123456789abcdef" for c in password)
            assert password_file.stat().st_mode & 0o777 == 0o440
            assert int(run("exec", "-T", "runtime", "id", "-u")) == 10001
            assert b"<html" in http("/")
            http("/", headers={"Host": f"mathdoc.test:{port}", "Origin": origin})
            http("/", headers={"Host": "untrusted.example"}, expected=403)
            http("/", headers={"Origin": "http://untrusted.example"}, expected=403)
            assert cli("status")["server"]["port"] == 17843
            assert "4.33.1" in run("exec", "-T", "runtime", "lean", "--version")
            assert run("exec", "-T", "runtime", "readlink",
                       "/var/lib/mdc/tools/lean/toolchains/leanprover--lean4---v4.33.1").strip() == "/opt/lean/4.33.1"
            # Password files are read by exec'd CLI processes, without entrypoint magic.
            for path, error in (("/missing-password", "read database password file"),
                                ("/dev/null", "database password file is empty")):
                run("exec", "-T", "-e", f"MDC_TERMINUS_PASSWORD_FILE={path}",
                    "runtime", "mdc", "status", error=error)
            run("exec", "-T", "-e", "MDC_TERMINUS_PASSWORD=conflicting-value",
                "runtime", "mdc", "status", error="set only one of")
            cli("init", "smoke")
            cli("start", "smoke/main")
            cli("branch", "new", "paused", "-p", "smoke/main")
            cli("start", "smoke/paused")
            cli("stop", "smoke/paused")
            node = cli("new", "-p", "smoke/main", "-t", "Container proof")
            wrapped = subprocess.run([str(deployment / "mdc"), "edit", "-p", "smoke/main",
                                      node["fnode"]], cwd=temporary, env=env,
                                     input="theorem container_proof : 1 + 1 = 2 := rfl\n",
                                     text=True, capture_output=True, check=True, timeout=60)
            assert json.loads(wrapped.stdout)["fnode"] == node["fnode"]
            checked = cli("lean", "check", "--build", "-p", "smoke/main", node["fnode"], timeout=360)
            assert checked["certified"] and checked["built"], checked
            preview = json.loads(http(f"/p/smoke/main/api/node/{node['fnode']}/latex/preview",
                                      {"source": r"\section{Runtime}A formula: $1+1=2$."}))
            assert preview["html"] and not preview["diagnostics"], preview
            run("exec", "-T", "runtime", "touch", "/var/lib/mdc/tools/persistence-marker")
            for action in ("restart", "recreate", "down-up"):
                print(f"Checking {action}, credentials and persistent state…", flush=True)
                if action == "restart":
                    run("restart")
                elif action == "down-up":
                    run("down")
                up(*(["--force-recreate"] if action == "recreate" else []))
                assert password_file.read_text().strip() == password
                states = cli("status")["projects"]
                assert states["smoke/main"]["running"]
                assert not states["smoke/paused"]["running"]
                assert cli("show", "-p", "smoke/main", node["fnode"])["title"] == "Container proof"
                checked = cli("lean", "check", "-p", "smoke/main", node["fnode"])
                assert checked["certified"] and checked["cache_hit"], checked
                run("exec", "-T", "runtime", "test", "-f", "/var/lib/mdc/tools/persistence-marker")
            assert password not in run("logs", "--no-color")
            for service in ("runtime", "database"):
                inspect = subprocess.run(["docker", "inspect", f"{project_name}-{service}"],
                                         text=True, capture_output=True, check=True)
                assert password not in inspect.stdout
            print("Checking lost-secret recovery without resetting the database…", flush=True)
            run("stop")
            password_file.rename(saved_password)
            failed = subprocess.run([*compose, "up", "-d", "--no-build", "--pull", "never",
                                     "--wait", "--wait-timeout", "15", "database"],
                                    env=env, cwd=temporary, capture_output=True, text=True, timeout=45)
            assert failed.returncode != 0
            assert not password_file.exists()
            assert "restore the original password file" in run("logs", "database")
            run("stop", "database")
            password_file.write_text("\n")
            failed = subprocess.run([*compose, "up", "-d", "--no-build", "--pull", "never",
                                     "--wait", "--wait-timeout", "15", "database"],
                                    env=env, cwd=temporary, capture_output=True, text=True, timeout=45)
            assert failed.returncode != 0
            assert "Database password file is empty" in run("logs", "database")
            run("stop", "database")
            password_file.unlink()
            saved_password.rename(password_file)
            # Restoring a file need not retain its original container group.
            password_file.chmod(0o600)
            os.chown(password_file, -1, os.getgid())
            up()
            assert password_file.stat().st_mode & 0o777 == 0o440
            assert cli("show", "-p", "smoke/main", node["fnode"])["title"] == "Container proof"
            print("Docker smoke passed: source-free startup, credentials, HTTP origins, CLI, bundled Lean, "
                  "LaTeX, restart, recreation, down/up, cache reuse and lost-secret recovery.", flush=True)
        except BaseException:
            subprocess.run([*compose, "logs", "--tail", "80"], env=env, timeout=30)
            raise
        finally:
            run("down", "--volumes", "--remove-orphans", timeout=120)


if __name__ == "__main__":
    main()
