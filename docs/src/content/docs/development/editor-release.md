---
title: Editor & Release
description: Maintain the VS Code extension, embedded frontend, documentation, and package versions.
---

## VS Code extension

`editors/vscode/` contains a declaration-only language extension for `.mdoc` files. It
registers the language, folding markers, a TextMate grammar, and embedded-language
mappings for source blocks.

Embedded highlighting recognizes source-type names case-insensitively and permits
metadata after the type, matching the `.mdoc` parser.

Install the checked-in package locally:

```bash
code --install-extension editors/vscode/mdc-mdoc-0.1.1.vsix --force
```

Build a VSIX from source:

```bash
cd editors/vscode
npx @vscode/vsce package
```

Publish through an authenticated publisher session:

```bash
cd editors/vscode
npx @vscode/vsce login mdc
npx @vscode/vsce publish
```

Or provide a token non-interactively:

```bash
cd editors/vscode
npx @vscode/vsce publish -p "$VSCE_PAT"
```

Before public publication, verify the `publisher`, bump the extension `version`, and add
Marketplace presentation metadata such as a README and icon.

## Embedded web assets

Any change under `web/src/` must be followed by:

```bash
cd web
npm ci
npm run check
npm test
npm run build
```

Commit the resulting `web/dist/` changes with the source. Release checking repeats the
build and rejects stale or untracked generated assets.

## Documentation site

Documentation source lives under `docs/src/content/docs/`. Validate it with:

```bash
cd docs
npm ci
npm run check
npm run build
```

`.github/workflows/docs-deploy.yml` deploys `main` to GitHub Pages at
`https://mathdocument.github.io/MathDoc/`. In repository settings, Pages must use
**GitHub Actions** as its source.

## Package versions

The Rust package version is declared in `Cargo.toml`; the VS Code extension has its own
version in `editors/vscode/package.json`. Bump only the artifact whose public behavior
is being released, and commit the corresponding lockfile or packaged output changes.
