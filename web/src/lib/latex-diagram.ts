import createDOMPurify from 'dompurify';
import { spawn } from 'threads';

const root = new URL('/tikz/1.0.0-beta24/', location.origin).href;
type TexWorker = { load(root: string): Promise<void>; texify(tex: string, options: Record<string, string>): Promise<string> };
let worker: Worker | undefined;
let runtime: Promise<TexWorker> | undefined;
let queue = Promise.resolve();
let texError = '';
const cache = new Map<string, string>();
const purifier = createDOMPurify(window);
// Diagrams can contain raw SVG specials. Keep local SVG references, never URLs
// or CSS escapes that could fetch content outside the bundled TeX environment.
purifier.addHook('uponSanitizeAttribute', (_node, attribute) => {
  const value = attribute.attrValue;
  if ((/href$/i.test(attribute.attrName) && !value.startsWith('#')) ||
      /\\|url\s*\(\s*["']?[^#\s"']/i.test(value)) attribute.keepAttr = false;
});

function reset() {
  worker?.terminate(); worker = undefined; runtime = undefined;
}
window.addEventListener('pagehide', reset);

function getRuntime() {
  if (!runtime) {
    // The upstream worker falls back to fetching arbitrary TeX filenames.
    // Permit only bundled, compressed runtime files, without cookies or redirects.
    const url = URL.createObjectURL(new Blob([`
      const root = ${JSON.stringify(root)}, nativeFetch = self.fetch;
      self.fetch = (input) => {
        const url = new URL(input, root);
        if (!url.href.startsWith(root) || !url.pathname.endsWith('.gz') || url.search || url.hash)
          return Promise.reject(new Error('TeX can only read bundled runtime files'));
        return nativeFetch(url, {credentials: 'omit', redirect: 'error'});
      };
      importScripts(root + 'run-tex.js');
    `], {type: 'text/javascript'}));
    worker = new Worker(url);
    worker.addEventListener('message', event => {
      if (typeof event.data === 'string' && event.data.startsWith('!') && !texError) texError = event.data;
    });
    runtime = spawn<TexWorker>(worker).then(async api => { await api.load(root.slice(0, -1)); return api; })
      .finally(() => URL.revokeObjectURL(url));
    if (!document.querySelector('link[data-tikz-fonts]')) {
      const link = document.createElement('link');
      link.rel = 'stylesheet'; link.href = root + 'fonts.css'; link.dataset.tikzFonts = '';
      document.head.append(link);
    }
  }
  return runtime!;
}

async function render(tex: string, preamble: string): Promise<string> {
  const key = JSON.stringify([tex, preamble]);
  if (cache.has(key)) return cache.get(key)!;
  let timeout: ReturnType<typeof setTimeout>;
  try {
    texError = '';
    const svg = await Promise.race([
      getRuntime().then(api => api.texify(tex, {texPackages: '{"tikz-cd":"","amsmath":"","amssymb":""}', addToPreamble: preamble, showConsole: 'true'})),
      new Promise<never>((_, reject) => { timeout = setTimeout(() => reject(new Error('Diagram rendering timed out')), 30000); }),
    ]);
    if (texError) throw new Error(texError);
    if (svg.length > 8 * 1024 * 1024) throw new Error('Diagram SVG exceeds 8 MiB');
    const clean = purifier.sanitize(svg, {USE_PROFILES: {svg: true}, FORBID_TAGS: ['foreignObject', 'image', 'style', 'animate', 'animateTransform', 'set']});
    if (!clean.includes('<svg')) throw new Error('TeX did not produce a diagram; check its source');
    cache.set(key, clean);
    // ponytail: keep the last 32 diagrams; a byte budget can replace this if large diagrams become common.
    if (cache.size > 32) cache.delete(cache.keys().next().value!);
    return clean;
  } catch (error) {
    reset(); throw new Error(texError || (error instanceof Error ? error.message : String(error)));
  } finally { clearTimeout(timeout!); }
}

export function renderDiagrams(host: HTMLElement) {
  for (const element of host.querySelectorAll<HTMLElement>('.latex-diagram[data-tex]')) {
    queue = queue.then(async () => {
      if (!element.isConnected) return;
      try {
        const svg = await render(element.dataset.tex!, element.dataset.preamble ?? '');
        if (element.isConnected) element.innerHTML = svg;
      } catch (error) {
        element.classList.add('latex-error');
        element.textContent = `Diagram: ${error instanceof Error ? error.message : String(error)}`;
      }
    });
  }
}
