---
title: Installation
---

Install Rust, Docker, Elan and the pinned Lean toolchain. MathDoc supports Unix hosts.

```sh
cargo build --release
elan toolchain install leanprover/lean4:v4.33.1
```

Run `python3 scripts/start-db.py` to start the pinned database with Docker and create a private `.env` if needed. Then run `python3 scripts/run-local.py init` once, and `python3 scripts/run-local.py serve`. Open `http://127.0.0.1:7599` in the local browser. The service embeds the committed frontend assets. For frontend development, rebuild them with `npm --prefix web ci` and `npm --prefix web run build`.

TerminusDB stores data in a persistent Docker volume. Keep that volume and the private password. `MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_CACHE_DIR` and `MDC_BIN` are optional overrides. CLI clients use `MDC_URL` and need no database credentials.
