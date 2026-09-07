"""Run local integration tests without putting database credentials in argv."""
import os
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
env = os.environ.copy()
config = root / ".env"
if config.exists():
    for line in config.read_text().splitlines():
        if line.startswith("MDC_") and "=" in line:
            key, value = line.split("=", 1)
            env.setdefault(key, value)
targets = sys.argv[1:] or ["test_database"]
for target in targets:
    if not target.startswith("test_") or not target.replace("_", "").isalnum():
        raise SystemExit("expected a Rust integration test target")
    subprocess.run(["cargo", "test", "--test", target, "--", "--ignored", "--nocapture"],
                   cwd=root, env=env, check=True)
