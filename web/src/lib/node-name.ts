/** Same qualified identifier grammar as store::validate_name. */
export function nodeNameError(name: string): string | null {
  const identifier = /^[\p{Alphabetic}_][\p{Alphabetic}\p{N}_']*$/u;
  return name.split('.').some(part => part === '_' || !identifier.test(part)) || name.split('.')[0].toLowerCase() === 'lakefile'
    ? "Use dot-separated identifiers, such as MX.Dot32.Exact (letters, digits, _ and ')."
    : null;
}
