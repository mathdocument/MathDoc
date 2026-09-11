---
title: Service and Lean configuration
---

Settings are read from `$XDG_CONFIG_HOME/mdc/config.toml`, defaulting to `~/.config/mdc/config.toml`. `MDC_CONFIG` selects an explicit file. Keep files containing credentials private (mode 600).

```toml
terminus_url = "http://127.0.0.1:6363"
terminus_user = "admin"
terminus_password = "the-existing-database-password"
# cache_dir = "/absolute/path/to/cache"
# lean_timeout_seconds = 300
```

Environment overrides are `MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_TERMINUS_PASSWORD`, `MDC_CACHE_DIR` and `MDC_LEAN_TIMEOUT_SECONDS`. `MDC_URL` and the TOML `url` setting are removed; delete an old `url` entry when upgrading. Client commands require `--proj DATABASE/BRANCH` after the command name (for example `mdc graph check --proj myproject/main`) and discover the running service through its local cache record. No default port or implicit project is used.

`init`, `start` and `status` need TerminusDB credentials. Clients and `stop` need only the same TerminusDB endpoint setting and local cache root as the running service; they do not query the database directly or require its password.

Caches default to `$XDG_CACHE_HOME/mdc` or `~/.cache/mdc`. The service appends the database endpoint hash, database name and branch. Cache overrides must be absolute paths, so moving the working directory cannot select a different cache.

Lean project settings are versioned separately in the database. Use the browser dialog or `mdc project show --proj myproject/main` / `mdc project set --proj myproject/main`. Set reads JSON with `toolchain`, `lakefile` and optional `manifest` string fields. Pin a Lean release, declare `lean_lib Lib`, and lock Git libraries to full commits in the complete manifest. Mutable path dependencies are rejected. `--prof` is a global option that prints command timing measurements to stderr without changing JSON output on stdout.
