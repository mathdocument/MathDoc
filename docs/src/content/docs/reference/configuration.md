---
title: Lean environments and configuration
---

Use the browser's Lean project dialog or `mdc project show` / `mdc project set`. Set reads JSON on stdin with `toolchain`, `lakefile` and optional `manifest` string fields.

The toolchain is pinned, such as `leanprover/lean4:v4.33.1`. Lake TOML must declare the managed `Lib` library. External libraries must be locked to full Git commits in a complete `lake-manifest.json`; mutable path dependencies are rejected. Libraries need not be Mathlib. An environment change selects a separate cache directory.

`MDC_TERMINUS_PASSWORD` is required by init/serve. Optional server settings include `MDC_TERMINUS_URL`, `MDC_TERMINUS_USER` and `MDC_CACHE_DIR`. CLI clients use `MDC_URL`. `--prof` measures client request time; it no longer reports filesystem refresh phases.
