---
title: Installation
---

Install Rust, Docker and Elan on a Unix host, then install one executable:

```sh
cargo install --path . --locked
elan toolchain install leanprover/lean4:v4.33.1
```

For a new database server, set a private `MDC_TERMINUS_PASSWORD` and run `docker compose up -d` with the supplied `compose.yaml`. Reuse an existing TerminusDB instance. The persistent volume is `mathdoc-terminus-data`; keep both the volume and credentials.

Store service settings in the private user configuration described in [Configuration](../../reference/configuration/). No project directory or startup wrapper is required.

```sh
mdc init myproject
mdc serve myproject
```

Run `init` once per new project. It creates a project database and schema on the already-running TerminusDB instance. Open `http://127.0.0.1:7599` while `serve` is running. The executable embeds the browser assets. To update it after frontend changes, rebuild `web/dist` before reinstalling.
