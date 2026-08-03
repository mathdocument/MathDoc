# MathDoc

[Documentation](https://mathdocument.github.io/MathDoc/) · [License](LICENSE)

MathDoc (`mdc`) is a plain-text knowledge system for connected mathematics. It
organizes `.mdoc` files as a dependency graph and keeps informal text, LaTeX, Lean,
Rocq, and Python source beside the ideas they describe.

Use the CLI, terminal graph browser, or local web interface to navigate and edit a
workspace. Stable language mirrors let native editors and compilers work normally,
while snapshot-aware synchronization preserves conflicting changes instead of silently
overwriting them.

## Quick start

```bash
cargo build --release
install -m 755 target/release/mdc "$HOME/.local/bin/mdc"

mkdir my-mathematics && cd my-mathematics
mdc init
mdc new -t "Background Lemma" -f notes/background
mdc new -t "Main Theorem" -f notes/theorem
mdc dep add notes/theorem --target notes/background
mdc serve notes/theorem
```

MathDoc currently supports Unix platforms. See the
[full documentation](https://mathdocument.github.io/MathDoc/) for the file format,
source workflow, complete CLI reference, architecture, and development guide.

## License

[MIT](LICENSE)
