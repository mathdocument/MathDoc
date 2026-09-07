"""Start the local MathDoc service/CLI using the repository's private .env."""
import os
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[1]
config = root / ".env"
if config.exists():
    for line in config.read_text().splitlines():
        if line.startswith("MDC_") and "=" in line:
            key, value = line.split("=", 1)
            os.environ.setdefault(key, value)
binary = os.environ.get("MDC_BIN", str(root / "target/release/mdc"))
os.chdir(root)
os.execvpe(binary, [binary, *(sys.argv[1:] or ["serve"])], os.environ)
