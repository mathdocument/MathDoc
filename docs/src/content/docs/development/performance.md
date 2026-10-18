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
python3 perf/graph-service.py --url http://127.0.0.1:17843/p/myproject/main \
  --cli coordinator/dist/cli.js --proj myproject/main --query Algebra \
  --samples 20 --output /tmp/graph-baseline.json
```

`MDC_TOKEN` authenticates the HTTP requests and is passed on to the CLI. `--cli`
is the path of an executable `mdc` (the backend build marks
`coordinator/dist/cli.js` executable). Set `MDC_URL` if the server is not on the
default origin. With `--cli`, `--proj` is required and must name the branch
served at `--url`; each CLI sample is a fresh process, so it includes Node
startup. `--query` (default `equation`) is the search term; `--samples` defaults
to 20, after one untimed warm-up request per endpoint.

The default run is read-only. `--writes` adds temporary nodes and measures
creation, renames, text saves, adding and removing edges, and rejection of stale
revisions and cycles. Use it only on a disposable branch
(`mdc branch new bench -p DB/main`, then `mdc branch del -p DB/bench`); the script
does not delete what it creates.

HTTP timings include connection setup, transfer and JSON decoding. No operating
system or database caches are flushed; the first request to a branch also loads
its snapshot. These are local observations, not latency guarantees.

## Collaboration load

`perf/board-load.ts` starts a coordinator (`coordinator/src/main.ts serve`) on a
fresh PostgreSQL schema and TerminusDB database, writes a synthetic project board
with N goals, and runs K agent processes that each repeat claim, two heartbeats,
submit and finish the way `mdc` sends them (`If-Match: *`; only a claim reads the
board first, to choose a task). No worker runs, so `submit` only queues a job and
LeanGround is never called. It prints, per command, the latency an agent sees
(p50, p95, max, including that read and any retry), throughput, revision
conflicts, lost claims, errors and board size, and whether every command's p95
meets `--target-p95`. It needs `MDC_TEST_DATABASE_URL`, `MDC_TEST_TERMINUS_URL`
and `MDC_TEST_TERMINUS_PASSWORD`.

`perf/sync-cost.ts` times the worker's sync job on a board with F certificates,
each read again from the LeanGround double on loopback; no branch is attached. It
needs only `MDC_TEST_DATABASE_URL`. Both remove what they create.

```sh
npx tsx perf/board-load.ts --agents 20 --goals 1000 --rounds 10 --target-p95 500 [--seed 1]
npx tsx perf/sync-cost.ts --facts 1000 [--runs 3]
```

The values shown are the defaults, except `--facts` (default 358).

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
change that needs them. `MDC_PERF_SAMPLES` sets the samples per metric (default
5). The committed baseline dates from 2026-09-07, before the Lean editor and
Infoview were removed, so local comparisons are against that older build.

Timings vary across machines, so pull requests benchmark the base and head
revisions on the same GitHub runner and apply the tolerances in
`web/perf/budgets.json`: at most 8% growth of the shell and of the total payload,
with absolute ceilings of 57,344 bytes for the shell and 9,000,000 bytes in total,
and 15–20% for runtime metrics. Lower is better for every metric. CI uploads both
raw reports.
