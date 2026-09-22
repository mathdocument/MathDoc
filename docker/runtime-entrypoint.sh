#!/bin/sh
set -eu

# Register the bundled release without copying it into a persistent volume.
# Project lean-toolchain files still select their own (possibly other) versions.
mkdir -p "$ELAN_HOME/toolchains"
bundled="$ELAN_HOME/toolchains/leanprover--lean4---v4.33.1"
if [ ! -e "$bundled" ]; then
    ln -sfn /opt/lean/4.33.1 "$bundled"
fi
if [ ! -f "$ELAN_HOME/settings.toml" ]; then
    elan default leanprover/lean4:v4.33.1
fi

exec mdc "$@"
