#!/bin/sh
set -eu

# Register the bundled release without copying it into a persistent volume.
# Project lean-toolchain files still select their own (possibly other) versions.
mkdir -p "$ELAN_HOME/toolchains"
toolchain=$(cat /opt/lean-toolchain)
version=${toolchain##*:v}
bundled="$ELAN_HOME/toolchains/leanprover--lean4---v$version"
if [ ! -e "$bundled" ]; then
    ln -sfn "/opt/lean/$version" "$bundled"
fi
if [ ! -f "$ELAN_HOME/settings.toml" ]; then
    elan default "$toolchain"
fi

# An upgraded image no longer contains the old symlink target. Move its
# default first, then let Elan install the old release if a project requests it.
for link in "$ELAN_HOME"/toolchains/leanprover--lean4---v*; do
    [ -L "$link" ] && [ ! -e "$link" ] || continue
    previous=${link##*---v}
    [ "$(readlink "$link")" = "/opt/lean/$previous" ] || continue
    selected=$(python3 -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb")).get("default_toolchain", ""))' "$ELAN_HOME/settings.toml")
    if [ "$selected" = "leanprover/lean4:v$previous" ]; then
        elan default "$toolchain"
    fi
    rm -- "$link"
done

exec mdc "$@"
