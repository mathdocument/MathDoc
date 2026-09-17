import type { LatexSession } from './latex-session.svelte';
import { fuzzyScore, fuzzyScoreGracefulAggressive } from 'vscode/vscode/vs/base/common/filters';

function filter<T extends {label: string; search: string}>(items: T[], query: string) {
  const word = query.trimStart(), lower = word.toLowerCase();
  // Monaco's default completion scoring, including its large-catalog cutoff for
  // typo correction. Equal scores use its label order (no locality/snippet bonus).
  const score = items.length > 2000 ? fuzzyScore : fuzzyScoreGracefulAggressive;
  return items.map(item => ({item, score: word
    ? score(word, lower, 0, item.search, item.search.toLowerCase(), 0)?.[0] : 0}))
    .filter((match): match is {item: T; score: number} => match.score !== undefined)
    .sort((a, b) => b.score - a.score || (a.item.label < b.item.label ? -1 : a.item.label > b.item.label ? 1 : 0))
    // Bound DOM size only after ranking the entire catalog.
    .slice(0, 50).map(match => match.item);
}

/** Completion data is scoped to this node's dependencies and the shared bibliography. */
export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, before: string) {
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    if (reference[1] === 'cite') {
      const options = (session.catalog?.citations ?? []).map(cite => ({
        label: cite.key, search: `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`,
        detail: [cite.title, cite.authors, cite.year].filter(Boolean).join(' · '), insert: cite.key,
      }));
      return {length: fragment.trimStart().length, options: filter(options, fragment)};
    }
    const options = session.references.map(ref => ({label: ref.name || ref.label, search: `${ref.label} ${ref.name} ${ref.title}`, detail: [ref.title, ref.label].filter(Boolean).join(' · '), insert: ref.label}));
    return {length: fragment.trimStart().length, options: filter(options, fragment)};
  }
  const environment = /\\(?:begin|end)\{([A-Za-z*]*)$/.exec(before);
  const command = /\\([A-Za-z]*)$/.exec(before);
  const match = environment ?? command;
  if (!match) return null;
  const options = (environment ? session.catalog?.environments : session.catalog?.commands ?? [])?.map(label => ({label, search: label, detail: '', insert: label})) ?? [];
  return {length: match[1].length, options: filter(options, match[1])};
}
