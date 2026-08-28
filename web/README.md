# Frontend (mdc serve)

Svelte 5 + Vite + TypeScript. Built output is embedded into the `mdc`
binary at compile time via `rust-embed`.

## Development

Two terminals:

```bash
# 1) Ordinary backend, on Vite's default proxy target
cargo run -- serve --bind 127.0.0.1:7599

# 2) Vite dev server
cd web && npm ci && npm run dev
```

Point your browser at the Vite dev URL (default http://localhost:5173); it
owns frontend serving and HMR in development and proxies `/api` to the Rust backend.

## Release build

```bash
cd web && npm install && npm run build   # writes web/dist/
cargo build --release                    # embeds web/dist into the binary
```

`web/dist/` is committed. Commit rebuilt assets with the frontend source; CI rebuilds
them and rejects drift.

The release binary has zero runtime dependency on Node.js.
