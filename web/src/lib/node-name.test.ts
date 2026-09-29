import { expect, test } from 'vitest';
import { leanImportName, nodeNameError } from './node-name';

test('node names use unquoted qualified identifiers', () => {
  for (const name of ['A', 'MX.Dot32.Exact', '数学.引理', 'A._private', "A.x'"]) expect(nodeNameError(name)).toBeNull();
  for (const name of ['', ' ', 'A B', 'A..B', '.A', 'A.', 'A/B', 'A::B', 'A.1', 'A.«B»', '_', 'A._', 'lakefile', 'LakeFile.X']) expect(nodeNameError(name)).not.toBeNull();
});

test('bare Lean names are escaped without duplicating its keyword table', () => {
  expect(leanImportName('def')).toBe('«def»');
  expect(leanImportName('Type')).toBe('«Type»');
  expect(leanImportName('A.def')).toBe('A.def');
});
