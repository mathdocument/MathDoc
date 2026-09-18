import { version } from 'mathjax/package.json';

interface MathJax {
  startup: { promise: Promise<void> };
  typesetPromise(elements: HTMLElement[]): Promise<void>;
  typesetClear(elements: HTMLElement[]): void;
}

const root = `/mathjax/${version}`;
const global = window as typeof window & { MathJax?: MathJax | Record<string, unknown> };
let runtime: Promise<MathJax> | undefined;

function loadMathJax(): Promise<MathJax> {
  if (runtime) return runtime;
  const script = document.createElement('script');
  runtime = new Promise<MathJax>((resolve, reject) => {
    global.MathJax = {
      loader: {
        paths: { mathjax: root, fonts: `${root}/fonts` },
        load: ['ui/safe', 'a11y/assistive-mml'],
        failed: reject,
      },
      startup: { typeset: false },
      output: { font: 'mathjax-modern', mtextInheritFont: true },
      chtml: { matchFontHeight: false },
      tex: { require: { allow: { html: false, texhtml: false, setoptions: false } } },
      options: {
        enableMenu: false,
        menuOptions: { settings: { enrich: false, speech: false, braille: false, assistiveMml: true } },
        safeOptions: { allow: { URLs: 'none', classes: 'none', cssIDs: 'none', styles: 'safe' } },
      },
    };
    script.src = `${root}/tex-chtml-nofont.js`;
    script.onload = () => {
      const mathjax = global.MathJax as MathJax;
      mathjax.startup.promise.then(() => resolve(mathjax), reject);
    };
    script.onerror = () => reject(new Error('Unable to load the math renderer'));
    document.head.append(script);
  }).catch(error => {
    script.remove();
    runtime = undefined;
    throw error;
  });
  return runtime;
}

/** Render only the backend's math placeholders; release them when the preview changes. */
export function renderMath(host: HTMLElement): () => void {
  const elements = [...host.querySelectorAll<HTMLElement>('.latex-math[data-tex]')];
  let disposed = false;
  let finished = false;
  let mathjax: MathJax | undefined;
  if (elements.length) {
    for (const element of elements) element.setAttribute('aria-busy', 'true');
    void loadMathJax().then(async renderer => {
      if (disposed) return;
      mathjax = renderer;
      for (const element of elements) {
        const tex = element.dataset.tex ?? '';
        element.textContent = element.dataset.display === 'true' ? `\\[${tex}\\]` : `\\(${tex}\\)`;
      }
      await renderer.typesetPromise(elements);
      finished = true;
      if (disposed) renderer.typesetClear(elements);
      else for (const element of elements) element.removeAttribute('aria-busy');
    }).catch(error => {
      finished = true;
      mathjax?.typesetClear(elements);
      if (disposed) return;
      for (const element of elements) {
        element.removeAttribute('aria-busy');
        element.classList.add('latex-error');
        element.textContent = `Math: ${error instanceof Error ? error.message : String(error)}`;
      }
    });
  }
  return () => { disposed = true; if (finished) mathjax?.typesetClear(elements); };
}
