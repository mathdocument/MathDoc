// LaTeX preview worker (src/latex/runtime.rs): one lazy Python process per branch,
// JSON lines in both directions, bounded concurrency and a per-request time limit.
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { DocError } from "./names.js";
import { type LatexProject } from "./model.js";

const MAX_FRAME = 32 * 1024 * 1024;
const SLOTS = 8;

/** The renderer sources stay in src/latex until the legacy build no longer embeds them. */
function rendererDir() {
  // coordinator/src/docs (or coordinator/dist/docs) -> repository root.
  return (
    process.env.MDC_RENDERER_DIR ??
    resolve(dirname(fileURLToPath(import.meta.url)), "../../../src/latex")
  );
}
function cacheRoot() {
  const root =
    process.env.MDC_CACHE_DIR ??
    join(process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "mathdoc");
  if (!root.startsWith("/"))
    throw new DocError("cache_dir / MDC_CACHE_DIR must be an absolute path");
  return root;
}

let installing: Promise<string> | null = null;
/** MDC_LATEX_PYTHON, or a pinned runtime installed once for all branches. */
async function python(): Promise<string> {
  if (process.env.MDC_LATEX_PYTHON) return process.env.MDC_LATEX_PYTHON;
  return (installing ??= (async () => {
    const runtime = join(cacheRoot(), "runtime", "plastex-3.1-pybtex-0.26.1");
    const bin = join(runtime, "bin/python");
    if (existsSync(join(runtime, ".ready"))) return bin;
    await mkdir(join(cacheRoot(), "runtime"), { recursive: true });
    const run = (cmd: string, args: string[]) =>
      new Promise<void>((ok, fail) => {
        const child = spawn(cmd, args, { stdio: ["ignore", "ignore", "pipe"] });
        let err = "";
        child.stderr.on("data", (d) => (err += d));
        const timer = setTimeout(() => child.kill(), 180000);
        child.on("error", () =>
          fail(
            new DocError("LaTeX preview requires Python 3 with venv support"),
          ),
        );
        child.on("exit", (code) => {
          clearTimeout(timer);
          code === 0
            ? ok()
            : fail(new DocError(`install LaTeX runtime: ${err}`));
        });
      });
    const requirements = (
      await readFile(join(rendererDir(), "requirements.txt"), "utf8")
    )
      .split("\n")
      .filter(Boolean);
    await run("python3", ["-m", "venv", runtime]);
    await run(bin, [
      "-m",
      "pip",
      "install",
      "--disable-pip-version-check",
      "--no-input",
      "--no-cache-dir",
      ...requirements,
    ]);
    await writeFile(join(runtime, ".ready"), "1");
    return bin;
  })().finally(() => (installing = null)));
}

class Worker {
  private buffer = Buffer.alloc(0);
  private waiting: ((line: Buffer | Error) => void) | null = null;
  projectKey: string | null = null;
  constructor(private child: ChildProcessWithoutNullStreams) {
    child.stdout.on("data", (chunk: Buffer) => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      if (this.buffer.length > MAX_FRAME)
        this.fail(
          new DocError("LaTeX renderer stopped or exceeded its response limit"),
        );
      this.flush();
    });
    child.on("exit", () =>
      this.fail(
        new DocError("LaTeX renderer stopped or exceeded its response limit"),
      ),
    );
    child.on("error", () => this.fail(new DocError("start LaTeX renderer")));
    // A renderer that failed to start (or died) must not crash the server via EPIPE.
    child.stdin.on("error", () =>
      this.fail(
        new DocError("LaTeX renderer stopped or exceeded its response limit"),
      ),
    );
  }
  static async spawn(): Promise<Worker> {
    const dir = rendererDir();
    const [richtext, renderer, bst] = await Promise.all(
      ["richtext.py", "renderer.py", "amsalpha.bst"].map((f) =>
        readFile(join(dir, f), "utf8"),
      ),
    );
    // Keep the bundled worker modules importable under Python's isolated mode.
    const source = `import sys, types\nm = types.ModuleType('richtext')\nsys.modules['richtext'] = m\nexec(${JSON.stringify(richtext)}, m.__dict__)\n${renderer}`;
    const child = spawn(await python(), ["-I", "-B", "-u", "-c", source, bst], {
      cwd: tmpdir(),
      stdio: ["pipe", "pipe", "ignore"],
    }) as unknown as ChildProcessWithoutNullStreams;
    return new Worker(child);
  }
  private flush() {
    const end = this.buffer.indexOf(10);
    if (end < 0 || !this.waiting) return;
    const line = this.buffer.subarray(0, end);
    this.buffer = this.buffer.subarray(end + 1);
    const w = this.waiting;
    this.waiting = null;
    w(line);
  }
  private fail(error: Error) {
    const w = this.waiting;
    this.waiting = null;
    w?.(error);
  }
  kill() {
    this.child.kill();
  }
  async request(
    project: LatexProject,
    request: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const key = request.project_key;
    if (typeof key !== "string")
      throw new DocError("LaTeX project key required");
    // A branch's worker retains its configuration; a restarted worker gets it again.
    const frame = Buffer.from(
      JSON.stringify(
        this.projectKey === key ? request : { ...request, project },
      ) + "\n",
    );
    if (frame.length >= MAX_FRAME)
      throw new DocError("LaTeX dependency context exceeds 32 MiB");
    const line = await new Promise<Buffer | Error>((ok) => {
      this.waiting = ok;
      if (this.child.exitCode !== null || this.child.stdin.destroyed)
        return this.fail(new DocError("start LaTeX renderer"));
      this.child.stdin.write(frame);
      this.flush();
    });
    if (line instanceof Error) throw line;
    let response: Record<string, unknown>;
    try {
      response = JSON.parse(line.toString("utf8"));
    } catch {
      throw new DocError("invalid LaTeX renderer response");
    }
    this.projectKey = key;
    return response;
  }
}

export class LatexService {
  private worker: Worker | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private active = 0;
  private stopped = false;

  async request(
    project: LatexProject,
    request: Record<string, unknown>,
  ): Promise<unknown> {
    if (this.stopped) throw new DocError("LaTeX service stopped");
    if (this.active >= SLOTS)
      throw new DocError("LaTeX preview is busy; retry shortly");
    this.active++;
    const run = this.queue.then(async () => {
      this.worker ??= await Worker.spawn();
      const worker = this.worker;
      let timer: NodeJS.Timeout | undefined;
      try {
        return await Promise.race([
          worker.request(project, request),
          new Promise<never>((_, fail) => {
            timer = setTimeout(
              () =>
                fail(
                  new DocError(
                    "LaTeX preview exceeded 10 seconds; shorten the block or check recursive macros",
                  ),
                ),
              10000,
            );
          }),
        ]);
      } catch (e) {
        worker.kill();
        if (this.worker === worker) this.worker = null;
        throw e;
      } finally {
        clearTimeout(timer);
      }
    });
    this.queue = run.catch(() => undefined);
    try {
      const response = (await run) as Record<string, unknown>;
      if (typeof response.error === "string")
        throw new DocError(response.error);
      return response.result;
    } finally {
      this.active--;
    }
  }

  shutdown() {
    this.stopped = true;
    this.worker?.kill();
    this.worker = null;
  }
}
