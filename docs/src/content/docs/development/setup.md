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
npm test
npm run build
```

Release builds embed the committed `web/dist/` output. Any frontend source change must
include a fresh production build. The build intentionally bundles only the five
supported source languages and one syntax theme.

For live frontend development, run the Rust API and Vite in separate terminals:

```bash title="Terminal 1"
cargo run -- serve --bind 127.0.0.1:7599
```

```bash title="Terminal 2"
cd web
npm run dev
```

Open the URL printed by Vite, normally port 5173. Vite owns frontend serving and HMR in
development; requests under `/api` are proxied to `$MDC_API_PROXY`, defaulting to
`http://127.0.0.1:7599`. The ordinary Rust server provides the API and keeps the same
embedded-asset path used by production builds.

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

## VS Code extension commands

Package and install the declaration-only extension locally:

```bash
cd editors/vscode
npx @vscode/vsce package
code --install-extension mdc-mdoc-0.1.1.vsix --force
```

The archive name is derived from the version in `package.json`. The release workflow
compares the packaged manifest, license, language configuration, and grammar byte for
byte with these source files.

## Release check

On pushes and pull requests, `.github/workflows/release-check.yml` performs:

1. Reproducible frontend dependency installation, type checking, tests, and build.
2. Verification that `web/dist/` matches committed frontend source.
3. Verification of the checked-in VS Code extension package.
4. Documentation dependency installation, Astro checking, and static build.
5. One Rust CI command: `cargo +stable test --locked`.

Documentation deployment is separate: pushes to `main` that touch `docs/` run the
official Astro GitHub Pages action.
