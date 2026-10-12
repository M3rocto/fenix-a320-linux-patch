#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ -x ./fenix-patch ]]; then
    exec ./fenix-patch "$@"
fi
# A release contains the installer as a program. A source checkout builds it.
# The installer reads bundle.json next to itself, so it is run from here.
if command -v cargo >/dev/null 2>&1; then
    cargo build --quiet --release --locked --bin fenix-patch
    ln -f target/release/fenix-patch .fenix-patch-dev 2>/dev/null || cp -f target/release/fenix-patch .fenix-patch-dev
    exec ./.fenix-patch-dev "$@"
fi
echo 'Fenix patch: this folder has no installer program. Download the Linux installer ZIP from the releases, or install Rust and run this script again.' >&2
exit 1
