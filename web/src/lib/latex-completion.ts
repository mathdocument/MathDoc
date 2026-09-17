import type { LatexSession } from './latex-session.svelte';

function filter<T extends {search: string}>(items: T[], query: string) {
  const words = query.trim().toLocaleLowerCase().split(/\s+/);
  return items.filter(item => words.every(word => item.search.toLocaleLowerCase().includes(word))).slice(0, 50);
}

/** Completion data is scoped to this node's dependencies and the shared bibliography. */
export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, before: string) {
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    if (reference[1] === 'cite') {
      const words = fragment.trim().toLocaleLowerCase().split(/\s+/);
      const options = [];
      for (const cite of session.catalog?.citations ?? []) {
        const search = `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`;
        const normalized = search.toLocaleLowerCase();
        if (!words.every(word => normalized.includes(word))) continue;
        options.push({label: cite.key, search, detail: [cite.title, cite.authors, cite.year].filter(Boolean).join(' · '), insert: cite.key});
        if (options.length === 50) break;
      }
      return {length: fragment.trimStart().length, options};
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
