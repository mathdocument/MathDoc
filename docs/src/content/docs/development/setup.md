---
title: Development Setup
description: Build, test, lint, and run MathDoc's Rust and Svelte codebases locally.
---

Development currently requires Unix. Release checks use stable Rust and Node 24.

## Rust commands

```bash
cargo build                     # debug build
cargo build --release           # optimized binary at target/release/mdc
cargo test                      # unit and integration tests
cargo test <name>               # tests matching a name substring
cargo test --test test_indcache # one integration test target
cargo fmt                       # format Rust
cargo clippy                    # lint Rust
```

Integration tests live in `tests/`; unit tests are colocated with source modules.

## Web frontend commands

The browser UI embedded by `mdc serve` is a separate Svelte 5 and Vite project:

```bash
cd web
npm ci
npm run check
npm run build
```

Release builds embed the committed `web/dist/` output. Any frontend source change must
include a fresh production build. The build intentionally bundles only the five
supported source languages and one syntax theme.

For live frontend development, run the Rust API and Vite in separate terminals:

```bash title="Terminal 1"
cargo run --features dev-web -- serve --bind 127.0.0.1:7599 --no-open
```

```bash title="Terminal 2"
cd web
npm run dev
```

With `dev-web`, `tower-http::ServeDir` serves `$MDC_WEB_DIR/dist`, defaulting to
`web/dist`. Vite normally uses port 5173 and falls back if occupied. Requests under
`/api` are proxied to `$MDC_API_PROXY`, defaulting to `http://127.0.0.1:7599`.

## Documentation commands

The public documentation is an Astro Starlight project:

```bash
cd docs
npm ci
npm run check
npm run build
npm run dev
```

The site uses `/MathDoc` as its production base path for GitHub Pages. The development
server prints the corresponding local URL.

## Release check

On pushes and pull requests, `.github/workflows/release-check.yml` performs:

1. Reproducible frontend dependency installation, type checking, and build.
2. Verification that `web/dist/` matches committed frontend source.
3. Documentation dependency installation, Astro checking, and static build.
4. `cargo +stable test --locked` for all Rust tests.

Documentation deployment is separate: pushes to `main` that touch `docs/` run the
official Astro GitHub Pages action.
