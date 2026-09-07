---
title: Development setup
---

```sh
npm --prefix web ci
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
cargo test --locked
python3 scripts/test-local.py test_database test_service test_lean_service
npm --prefix web run test:e2e
```

The integration tests need local TerminusDB, private `.env`, native Lean v4.33.1 and Playwright Chromium. They create independent test databases. The optional `test_service_scale` target measures real database-backed requests on 47,435 nodes and 368,017 edges.

Frontend development uses Vite with `MDC_API_PROXY` pointing to the local service. Production builds embed `web/dist` in the Rust binary. See the repository README for local deployment.
