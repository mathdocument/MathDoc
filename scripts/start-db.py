"""Start the pinned local database with Docker CLI (Compose is optional)."""
import os
from pathlib import Path
import re
import secrets
import subprocess
import time
import urllib.error
import urllib.request

root = Path(__file__).resolve().parents[1]
config = root / ".env"
if not config.exists():
    descriptor = os.open(config, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(f"MDC_TERMINUS_PASSWORD={secrets.token_urlsafe(32)}\n")
env = os.environ.copy()
for line in config.read_text().splitlines():
    if line.startswith("MDC_") and "=" in line:
        key, value = line.split("=", 1)
        env.setdefault(key, value)
if not env.get("MDC_TERMINUS_PASSWORD"):
    raise SystemExit("Set MDC_TERMINUS_PASSWORD in the private .env first.")
name = "mathdoc-terminusdb"
exists = subprocess.run(["docker", "container", "inspect", name],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
if exists:
    subprocess.run(["docker", "update", "--restart", "unless-stopped", name], check=True)
    subprocess.run(["docker", "start", name], check=True)
else:
    image = re.search(r"^\s+image:\s*(\S+)$", (root / "compose.yaml").read_text(), re.M)[1]
    env["TERMINUSDB_ADMIN_PASS"] = env["MDC_TERMINUS_PASSWORD"]
    subprocess.run([
        "docker", "run", "--detach", "--name", name, "--restart", "unless-stopped",
        "--publish", "127.0.0.1:6363:6363", "--env", "TERMINUSDB_ADMIN_PASS",
        "--env", "TERMINUSDB_SERVER_PORT=6363",
        "--volume", "mathdoc-terminus-data:/app/terminusdb/storage", image,
    ], env=env, check=True)
deadline = time.monotonic() + 60
while True:
    try:
        with urllib.request.urlopen("http://127.0.0.1:6363/api/ok", timeout=2):
            break
    except (urllib.error.URLError, TimeoutError):
        if time.monotonic() >= deadline:
            raise SystemExit("TerminusDB did not become ready; check docker logs mathdoc-terminusdb.")
        time.sleep(1)
print("TerminusDB: http://127.0.0.1:6363 (private password stays in .env)")
