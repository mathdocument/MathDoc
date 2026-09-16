import { readFile } from 'node:fs/promises';
import type { Plugin } from 'vite';
import type { Plugin as EsbuildPlugin } from 'esbuild';

// Monaco installs a blocking wheel observer even when mouseWheelZoom is off.
// Safari then cannot start native rubber-banding at a nested pane's boundary.
// Keep the observer, but make it passive when zoom is disabled (our configuration).
function patch(code: string) {
  const original = 'onMouseWheel, { capture: true, passive: false }';
  if (!code.includes(original)) throw new Error('Monaco wheel listener changed; review its passive configuration');
  return code.replace(original, 'onMouseWheel, { capture: true, passive: !this._context.configuration.options.get(EditorOption.mouseWheelZoom) }');
}
const modulePath = '/vs/editor/browser/controller/mouseHandler.js';
export const monacoWheel: Plugin = {
  name: 'monaco-passive-wheel', enforce: 'pre',
  transform(code, id) { if (id.endsWith(modulePath)) return patch(code); },
};
export const monacoWheelDeps: EsbuildPlugin = {
  name: 'monaco-passive-wheel',
  setup(build) {
    build.onLoad({filter: /\/mouseHandler\.js$/}, async ({path}) => {
      if (path.endsWith(modulePath)) return {contents: patch(await readFile(path, 'utf8')), loader: 'js'};
    });
  },
};
