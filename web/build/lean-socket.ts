import { readFile } from 'node:fs/promises';
import type { Plugin } from 'vite';
import type { Plugin as EsbuildPlugin } from 'esbuild';

// The installed wrapper never settles startup when the socket closes during
// initialize. A surviving editor must be able to finish shutdown and reconnect.
function patchLeanSocket(code: string) {
  const open = 'webSocket.onerror = (ev) => {';
  const close = 'await this.languageClient?.stop();';
  if (!code.includes(open) || !code.includes(close)) throw new Error('Lean socket wrapper changed; review its shutdown handling');
  return code.replace(open, `webSocket.addEventListener('close', () => reject(new Error('Lean connection closed during initialization')), {once: true});
                ${open}`).replace(close, `await this.languageClient?.stop().catch(error => this.logger?.info(String(error)));`);
}
const modulePath = '/monaco-editor-wrapper/dist/languageClientWrapper.js';
export const leanSocket: Plugin = {
  name: 'lean-socket-shutdown', enforce: 'pre',
  transform(code, id) { if (id.endsWith(modulePath)) return patchLeanSocket(code); },
};
export const leanSocketDeps: EsbuildPlugin = {
  name: 'lean-socket-shutdown',
  setup(build) {
    build.onLoad({filter: /\/languageClientWrapper\.js$/}, async ({path}) => {
      if (path.endsWith(modulePath)) return {contents: patchLeanSocket(await readFile(path, 'utf8')), loader: 'js'};
    });
  },
};
