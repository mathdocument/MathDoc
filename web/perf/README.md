# Frontend performance baseline

The benchmark runs the production build in Chromium at 1440x900 against fixed,
in-process API fixtures. It measures the shell and total compressed payload,
a 500-line LaTeX editor, and a 10,000-node / 19,993-edge graph. Browser timings
use a warm HTTP cache and report median values, except for the graph frame p95.

```sh
npx playwright install --no-shell chromium
npm run perf
```

`npm run perf` compares a new run with `baseline.json`. Run it on the same
machine when checking a local change. `npm run perf:measure` writes an
uncompared report to `latest.json`; that file is ignored. Use
`npm run perf:record` only when intentionally accepting a new baseline.

Runtime timings vary across machines. Pull requests therefore benchmark the
base and head revisions on the same GitHub runner and apply the tolerances in
`budgets.json`. Bundle budgets are also capped absolutely. CI uploads both raw
reports so an unexpected result can be inspected without rerunning it.

Changes to fixtures, budgets, or `baseline.json` should be reviewed alongside
the optimization that requires them. Lower values are better for every metric.
