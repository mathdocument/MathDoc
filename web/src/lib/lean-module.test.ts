import {expect, test} from 'vitest';
import {leanSourcePath} from './lean-module';

test('static and native Lean models use the same module URI', () => {
  expect(leanSourcePath('Mathlib.Algebra.Algebra.Defs')).toBe('/project/Mathlib/Algebra/Algebra/Defs.lean');
  expect(leanSourcePath('Lib.EGA.«1-1.7.1»')).toBe('/project/Lib/EGA/1-1.7.1.lean');
});
