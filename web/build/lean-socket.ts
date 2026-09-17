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
const patches: [string, (code: string) => string][] = [
  ['/monaco-editor-wrapper/dist/languageClientWrapper.js', patchLeanSocket],
  ['/lean4monaco/dist/vscode-lean4/vscode-lean4/src/leanclient.js', code => {
    const original = 'this.noPrompt = false;\n        this.progress = new Map();';
    if (!code.includes(original)) throw new Error('Lean client changed; review stop cleanup');
    // The native client clears its map but never tells the gutter to clear.
    return code.replace(original, `for (const uri of this.progress.keys()) {
            this.progressChangedEmitter.fire([uri.toString(), []]);
            this.diagnosticsEmitter.fire({uri: uri.toString(), diagnostics: []});
        }
        ${original}`);
  }],
  ['/lean4monaco/dist/infowebview.js', code => {
    const originals = [
      'this.iframe = document.createElement("iframe");',
      'this.themeService.onDidColorThemeChange(() => { this.updateCssVars(); });',
      "document.defaultView.addEventListener('message', m => {",
      'return new IFrameInfoWebview(this.iframe, rpc);',
    ];
    if (originals.some(text => !code.includes(text))) throw new Error('Lean Infoview factory changed; review disposal');
    // Each view must own its transport and unregister listeners on disposal.
    // Otherwise old RPC handlers consume messages from the next iframe.
    return code.replace(originals[0], `const iframe = this.iframe = document.createElement("iframe");`)
      .replace(originals[1], 'const theme = this.themeService.onDidColorThemeChange(() => { this.updateCssVars(); });')
      .replaceAll('this.iframe.contentWindow', 'iframe.contentWindow')
      .replace(originals[2], `const messages = new AbortController();\n        document.defaultView.addEventListener('message', m => {`)
      .replace(`});\n        ${originals[3]}`, `}, {signal: messages.signal});
        const view = new IFrameInfoWebview(iframe, rpc);
        view.onDidDispose(() => { messages.abort(); theme.dispose(); });
        return view;`);
  }],
];
export const leanSocket: Plugin = {
  name: 'lean-socket-shutdown', enforce: 'pre',
  transform(code, id) { return patches.find(([path]) => id.endsWith(path))?.[1](code); },
};
export const leanSocketDeps: EsbuildPlugin = {
  name: 'lean-socket-shutdown',
  setup(build) {
    build.onLoad({filter: /\/(languageClientWrapper|leanclient|infowebview)\.js$/}, async ({path}) => {
      const patch = patches.find(([suffix]) => path.endsWith(suffix))?.[1];
      if (patch) return {contents: patch(await readFile(path, 'utf8')), loader: 'js'};
    });
  },
};
