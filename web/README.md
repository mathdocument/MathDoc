# Frontend (mdc serve)

Svelte 5 + Vite + TypeScript. Built output is embedded into the `mdc`
binary at compile time via `rust-embed`.

## Development

Two terminals:

```bash
# 1) Backend with hot-reload feature (serves web/ via tower-http ServeDir)
cargo run --features dev-web -- serve

# 2) Vite dev server (HMR)
cd web && npm install && npm run dev
```

Point your browser at the Vite dev URL (default http://localhost:5173); it
proxies `/api` to the Rust backend.

## Release build

```bash
cd web && npm install && npm run build   # writes web/dist/
cargo build --release                    # embeds web/dist into the binary
```

The release binary has zero runtime dependency on Node.js.
