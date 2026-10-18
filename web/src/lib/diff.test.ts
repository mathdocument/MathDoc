import { describe, expect, it } from 'vitest';
import { lineDiff } from './diff';

const ops = (before: string, after: string) => lineDiff(before, after).map((l) => `${l.op}${l.text}`);

describe('line diff of a writeback preview', () => {
  it('keeps common lines and replaces the changed ones', () => {
    expect(ops('theorem t : P := by\n  sorry\n', 'theorem t : P := by\n  rw [h]\n  simp\n')).toEqual([
      ' theorem t : P := by', '-  sorry', '+  rw [h]', '+  simp',
    ]);
  });
  it('shows a new node as all added and an unchanged text as all kept', () => {
    expect(ops('', 'a\nb')).toEqual(['+a', '+b']);
    expect(ops('a\nb\n', 'a\nb')).toEqual([' a', ' b']);
  });
  it('finds lines kept between insertions and deletions', () => {
    expect(ops('a\nb\nc\nd', 'a\nx\nc\nd\ne')).toEqual([' a', '-b', '+x', ' c', ' d', '+e']);
  });
});
