---
title: Performance measurements
---

Run these from the source checkout against disposable data. Record the machine,
build mode, database size and whether caches were warm; compare results only on
the same machine.

## Graph API

`perf/graph-service.py` measures a running server over HTTP: graph check, roots,
full graph, searches, name/UUID resolution, node views, direct and transitive
dependencies and referrers, leaves, dependency candidates, IOR, history, and
eight concurrent readers. It records every sample, median, p95 and response
size. It never contacts LeanGround.

```sh
export MDC_TOKEN=your-access-token
python3 perf/graph-service.py --url http://127.0.0.1:17843/p/mathlib4/main \
  --cli coordinator/dist/cli.js --proj mathlib4/main --query Algebra \
  --samples 20 --output /tmp/graph-baseline.json
```

`MDC_TOKEN` authenticates the HTTP requests and is passed on to the CLI. `--cli`
is the path of the TypeScript `mdc`. The TypeScript build does not set the
executable bit, so run `chmod +x coordinator/dist/cli.js` once (in the Docker
image `mdc` is already executable). Set `MDC_URL` if the server is not on the
default origin. With
`--cli`, `--proj` is required and must name the branch served at `--url`; each
CLI sample is a fresh process, so it includes Node startup.

The default run is read-only. `--writes` adds temporary nodes and measures
creation, renames, text saves, adding and removing edges, and rejection of stale
revisions and cycles. Use it only on a disposable branch
(`mdc branch new bench -p DB/main`, then `mdc branch del -p DB/bench`); the script
does not delete what it creates.

HTTP timings include connection setup, transfer and JSON decoding. No operating
system or database caches are flushed; the first request to a branch also loads
its snapshot. These are local observations, not latency guarantees.

## Test graphs

`perf/mathlib-import.py` converts a pinned Mathlib checkout into an import bundle
(one node per module, source bytes preserved). It uses the checkout's Lean
toolchain through `elan` to parse imports, so it needs Elan on the machine that
runs it, not on the server. The resulting bundle is far larger than the API's
current request-body limit of 2,200,000 bytes, so it cannot be restored with
`mdc import` at present.

`perf/lean-block-scan.ts` reports how many Lean sources the node scanner accepts,
per file and per top-level command:

```sh
npx tsx perf/lean-block-scan.ts /path/to/lean/sources [--json]
```

[Archived reports](https://github.com/mathdocument/MathDoc/tree/main/perf/reports)
keep earlier measurements, including those of the removed Rust backend; their
commands refer to their original revisions.

## Frontend performance

The benchmark runs the production build in Chromium at 1440×900 against fixed,
in-process API fixtures. It measures the shell and total compressed payload, a
500-line LaTeX editor, and a 10,000-node / 19,993-edge graph. Browser timings use
a warm HTTP cache and report medians, except the graph frame p95. The same run
checks an 8,311-dependency list: bounded DOM size, scrolling to the end, keyboard
navigation, node selection and scroll preservation across view switches (timings
in `rawSamples.relations`). Canvas checks reject a blank frame on resize; a
3840×2160, 2× check reaches the canvas pixel cap and verifies the fixed 3/8
sidebar at several widths. The list check also covers dependency-removal
pagination, filtering, selection and the exact submitted IDs. Navigation checks
delay the shell script and require a header on the first frame, stable header
geometry, directory/project transitions, back and forward, theme persistence and
reduced motion.

```sh
npm exec -w mdc-web -- playwright install --no-shell chromium
npm run perf -w mdc-web
```

WebKit (Safari's engine), for interaction checks without comparing timings to
the Chromium baseline:

```sh
npm exec -w mdc-web -- playwright install webkit
node web/perf/run.mjs --browser webkit --output /tmp/mdc-webkit-perf.json
```

`npm run perf -w mdc-web` compares the run with `web/perf/baseline.json`.
`npm run perf:measure -w mdc-web` writes an uncompared `web/perf/latest.json`
(ignored by Git). Use `npm run perf:record -w mdc-web` only to accept a new
baseline intentionally, and review baseline or budget changes together with the
change that needs them.

Timings vary across machines, so pull requests benchmark the base and head
revisions on the same GitHub runner and apply the tolerances in
`web/perf/budgets.json`: at most 8% growth of the shell and of the total payload,
with absolute ceilings of 57,344 bytes for the shell and 9,000,000 bytes in total,
and 15–20% for runtime metrics. Lower is better for every metric. CI uploads both
raw reports.
