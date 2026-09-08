---
title: Service and Lean configuration
---

Settings are read from `$XDG_CONFIG_HOME/mdc/config.toml`, defaulting to `~/.config/mdc/config.toml`. `MDC_CONFIG` selects an explicit file. Keep files containing credentials private (mode 600).

```toml
url = "http://127.0.0.1:7599"
terminus_url = "http://127.0.0.1:6363"
terminus_user = "admin"
terminus_password = "the-existing-database-password"
# cache_dir = "/absolute/path/to/cache"
# lean_timeout_seconds = 300
```

Environment overrides are `MDC_URL`, `MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_TERMINUS_PASSWORD`, `MDC_CACHE_DIR` and `MDC_LEAN_TIMEOUT_SECONDS`. CLI `--url` takes precedence over the configured client URL. Clients need no TerminusDB credentials. Server/init commands select the database explicitly; a client URL selects the served project and branch.

Caches default to `$XDG_CACHE_HOME/mdc` or `~/.cache/mdc`. The service appends the database endpoint hash, database name and branch. Cache overrides must be absolute paths, so moving the working directory cannot select a different cache.

Lean project settings are versioned separately in the database. Use the browser dialog or `mdc project show` / `mdc project set`. Set reads JSON with `toolchain`, `lakefile` and optional `manifest` string fields. Pin a Lean release, declare `lean_lib Lib`, and lock Git libraries to full commits in the complete manifest. Mutable path dependencies are rejected. `--prof` reports client request timing.
