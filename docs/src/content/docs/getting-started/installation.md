---
title: Installation
description: Build MathDoc and install its optional editor and compiler integrations.
---

MathDoc currently supports Unix platforms. The `mdc` binary is built from source with
the stable Rust toolchain.

## Build the CLI

Clone the repository and create an optimized binary:

```bash
git clone https://github.com/mathdocument/MathDoc.git
cd MathDoc
cargo build --release
```

The binary is written to `target/release/mdc`. Put it on your `PATH`, or invoke it by
that path while evaluating the project.

```bash
install -m 755 target/release/mdc "$HOME/.local/bin/mdc"
mdc --help
```

:::note
MathDoc workspace discovery and safe file operations currently depend on Unix
filesystem behavior. Windows is not supported.
:::

## Optional compiler tools

Install only the native tools required by the source types you use.

| Source type | Required command | What MathDoc runs |
| --- | --- | --- |
| `text` | None | No compiler |
| `latex` | `latexmk`, `xelatex` | `latexmk -pdf -xelatex -interaction=nonstopmode -halt-on-error Main.tex` |
| `python` | `python3` or `python` | The selected mirror with `-B` |
| `lean` | `lake` | `lake --quiet --no-ansi build +Lib` |
| `rocq` | `rocq` | The selected `.v` module |

Missing optional compilers do not prevent use of the CLI, graph, editor, or other
source types. A missing compiler normally makes `mdc work` return exit code `127` for
that target.

## VS Code support

The repository includes a packaged extension for VS Code 1.85 or newer. It provides
syntax highlighting, folding, and embedded-language mapping in `.mdoc` files:

```bash
code --install-extension editors/vscode/mdc-mdoc-0.1.0.vsix --force
```

The extension is declaration-only and does not install `mdc` or any compiler.

## Verify the installation

Create a temporary directory and initialize an empty workspace:

```bash
mkdir mathdoc-demo
cd mathdoc-demo
mdc init
mdc graph check
```

A successful initialization creates a real `.mdc/` control directory and a commented
configuration template at `.mdc/config.toml`.
