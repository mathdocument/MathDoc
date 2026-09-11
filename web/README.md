# Frontend (mdc start)

Svelte 5 + Vite + TypeScript. Built output is embedded into the `mdc`
binary at compile time via `rust-embed`.

## Development

Use an existing development project, with caches outside the source checkout:

```bash
# Background backend, on Vite's default proxy target
mdc start dev/main --port 7599

# Vite dev server
cd web && npm ci && npm run dev
```

Point your browser at the Vite dev URL (default http://localhost:5173); it
owns frontend serving and HMR in development and proxies `/api` to the Rust backend.
Stop the backend with `mdc stop dev/main`.

## Release build

```bash
cd web && npm install && npm run build   # writes web/dist/
cargo build --release                    # embeds web/dist into the binary
```

`web/dist/` is committed. Commit rebuilt assets with the frontend source; CI rebuilds
them and rejects drift.

The release binary has zero runtime dependency on Node.js.

## Browser integration tests

From the repository root, run `npm --prefix web run build` and `cargo build --locked`.
Then run `npx playwright install --no-shell chromium` and `npm run test:e2e` from `web/`.
The tests start the real local backend in disposable workspaces. `MDC_BIN` optionally
selects another prebuilt binary.
