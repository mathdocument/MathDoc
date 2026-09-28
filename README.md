# MathDoc

MathDoc is a mathematical document and shared proof coordination application.
The default application is now **TypeScript + Svelte + PostgreSQL**. LeanGround
owns verified facts and final certification; dedicated verification components
own sketch checking. MathDoc owns collaboration, review and document versions.

## Run the coordination application

Requires Node.js 22+, PostgreSQL 17, and a configured LeanGround fact service.
Existing document editing additionally needs the existing TerminusDB instance.

```sh
npm ci
npm run check
npm test
npm run build
# Set environment variables from config.example.env through your shell/process manager.
npm run migrate
npm start
# In another terminal with the same environment:
npm run worker
```

Open http://127.0.0.1:17843 and use a token configured in `MDC_ACTORS`.
The default build does not install Rust, Lean, Lake, Python, Monaco or Infoview.

- [Architecture, API, migration and module removal](docs/coordination.md)
- [Configuration example](config.example.env)
- Container deployment: `docker compose -f compose.server.yaml up --build`
- Database integration tests: set `MDC_TEST_DATABASE_URL` to a dedicated PostgreSQL
  database and run `npm test`. Tests create and drop isolated schemas.

## Existing MathDoc document application

The previous Rust backend (`src/`) and full editor (`web/`) are retained as an
explicit legacy application during migration. They are not part of the root npm
workspace, default image or coordination runtime. Existing TerminusDB data is
read in place; it is not migrated or deleted.

```sh
npm --prefix web ci
npm --prefix web run build
cargo build --release --locked
./target/release/mdc start --foreground
```

Use a different port if running both applications. Legacy container:
`docker compose -f compose.legacy.yaml up --build`.
The detailed documentation site currently describes that legacy application;
use `docs/coordination.md` for the new default runtime and its current limits.
