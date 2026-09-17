import 'vscode/localExtensionHost';
import { initialize, getService, IConfigurationService } from 'vscode/services';
import { registerExtension } from 'vscode/extensions';
import getConfiguration from '@codingame/monaco-vscode-configuration-service-override';
import getTextmate from '@codingame/monaco-vscode-textmate-service-override';
import getTheme from '@codingame/monaco-vscode-theme-service-override';
import getLanguages from '@codingame/monaco-vscode-languages-service-override';
import getModels from '@codingame/monaco-vscode-model-service-override';
import { editor, languages, Uri } from 'monaco-editor';
import type { Theme } from './theme';

export const sourceOptions: editor.IStandaloneEditorConstructionOptions = {
  automaticLayout: false, fontFamily: 'JuliaMono', fontSize: 13, tabSize: 2,
  wordWrap: 'on', wrappingStrategy: 'simple', minimap: {enabled: false},
  folding: false, stickyScroll: {enabled: false}, lineNumbersMinChars: 1,
  lineDecorationsWidth: 5, scrollBeyondLastLine: false, mouseWheelZoom: false,
  scrollbar: {handleMouseWheel: false, vertical: 'hidden', horizontal: 'hidden'},
  fixedOverflowWidgets: true, renderLineHighlight: 'gutter',
};
let ready: Promise<void> | undefined;
export function initializeMonaco() {
  return ready ??= (async () => {
    window.MonacoEnvironment = {
      getWorker(_moduleId, label) {
        if (label === 'textMateWorker') return new Worker(new URL('@codingame/monaco-vscode-textmate-service-override/worker', import.meta.url), {type: 'module'});
        return new Worker(new URL('monaco-editor/esm/vs/editor/editor.worker.js', import.meta.url), {type: 'module'});
      },
    };
    await initialize({...getConfiguration(), ...getTextmate(), ...getTheme(), ...getLanguages(), ...getModels()}, document.body, {workspaceProvider: {trusted: true, workspace: {workspaceUri: Uri.file('/workspace.code-workspace')}, async open() {return false;}}});
    await (await import('@codingame/monaco-vscode-theme-defaults-default-extension')).whenReady;
    const font = new FontFace('JuliaMono', `url(${new URL('lean4monaco/dist/fonts/JuliaMono-Regular.ttf', import.meta.url)})`);
    document.fonts.add(font);
    await font.load();
  })();
}
export async function setMonacoTheme(theme: Theme) {
  return (await getService(IConfigurationService)).updateValue('workbench.colorTheme', theme === 'light' ? 'Default Light Modern' : 'Default Dark Modern');
}

const grammars = {
  text: () => import('@shikijs/langs/markdown'),
  latex: () => import('@shikijs/langs/latex'),
  rocq: () => import('@shikijs/langs/coq'),
};
const loaded = new Map<string, Promise<string>>();
/** Use the same native TextMate engine as Lean; the package supplies only grammars. */
export function loadSourceLanguage(type: keyof typeof grammars) {
  let pending = loaded.get(type);
  if (!pending) {
    pending = (async () => {
      await initializeMonaco();
      const definitions = (await grammars[type]()).default;
      const language = type === 'text' ? 'markdown' : type === 'rocq' ? 'coq' : 'latex';
      const extension = registerExtension({
        name: `mdc-${type}`, publisher: 'mathdoc', version: '1.0.0', engines: {vscode: '*'},
        contributes: {
          languages: [{id: language}],
          grammars: definitions.map(grammar => ({language: grammar.name === language ? language : undefined, scopeName: grammar.scopeName, path: `/${grammar.name}.json`})),
        },
      }, 1 /* LocalProcess */);
      for (const grammar of definitions) {
        extension.registerFileUrl(`/${grammar.name}.json`, URL.createObjectURL(new Blob([JSON.stringify(grammar)], {type: 'application/json'})));
      }
      await extension.whenReady();
      languages.setLanguageConfiguration(language, {
        brackets: [['{', '}'], ['[', ']'], ['(', ')']],
        autoClosingPairs: [{open: '{', close: '}'}, {open: '[', close: ']'}, {open: '(', close: ')'}],
        comments: type === 'latex' ? {lineComment: '%'} : type === 'rocq' ? {blockComment: ['(*', '*)']} : {blockComment: ['<!--', '-->']},
      });
      await editor.colorize('', language, {});
      return language;
    })();
    loaded.set(type, pending);
    void pending.catch(() => loaded.delete(type));
  }
  return pending;
}
