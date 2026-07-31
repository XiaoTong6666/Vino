#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0
#
# Build-only validation for the review workspace. This script deliberately has
# no install, module-load, service-management, hardware-access, bootloader, or
# reboot operation.

set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
kernel_tree="${KERNEL_TREE:?set KERNEL_TREE to the external Vino kernel checkout}"
kernel_base="${KERNEL_BASE:-integration/base-20260727}"
kernel_head="${KERNEL_HEAD:-series/vino-upstream}"
userspace_tree="${USERSPACE_TREE:-$workspace/revdi}"
jobs="${JOBS:-16}"

"$workspace/tools/regenerate-patches.sh"
"$workspace/tools/check-series.sh"

git -C "$kernel_tree" diff --check "$kernel_base..$kernel_head"

# The final six patches are the EVDI and Vino consumers. Generic prerequisite
# patches retain their subsystem authors and review state.
while IFS= read -r commit; do
    (
        cd "$kernel_tree"
        scripts/checkpatch.pl \
            --quiet --strict --ignore FILE_PATH_CHANGES \
            -g "$commit"
    )
done < <(git -C "$kernel_tree" rev-list --reverse "$kernel_head~6..$kernel_head")

if rg -n \
    '\bunsafe\s*\{|\bunsafe\s+(fn|impl|trait)|\bbindings::|Arc::into_raw|Arc::from_raw|AtomicPtr' \
    "$kernel_tree/drivers/gpu/drm/vino" \
    "$kernel_tree/drivers/gpu/drm/evdi" \
    --glob '*.rs'; then
    echo "error: a kernel consumer bypasses the safe Rust subsystem APIs" >&2
    exit 1
fi

expected_trailers=$'Assisted-by: Claude:claude-opus-5-0\nAssisted-by: Codex:gpt-5\nSigned-off-by: Mike Lothian <mike@fireburn.co.uk>'
while IFS= read -r commit; do
    trailers="$(
        git -C "$kernel_tree" show -s --format=%B "$commit" \
            | sed -e '${/^$/d;}' \
            | tail -n 3
    )"
    if [ "$trailers" != "$expected_trailers" ]; then
        echo "error: unexpected Mike trailer block in $commit" >&2
        exit 1
    fi
done < <(
    git -C "$kernel_tree" rev-list \
        --author='Mike Lothian <mike@fireburn.co.uk>' \
        "$kernel_base..$kernel_head"
)

if git -C "$kernel_tree" log \
    --author='Lyude Paul' \
    --format='%H%n%B%n--END--' \
    "$kernel_base..$kernel_head" \
    | rg '^(Assisted-by:|Signed-off-by: Mike Lothian|Co-developed-by: Mike Lothian)'; then
    echo "error: a Lyude-authored patch carries a Mike or assistance trailer" >&2
    exit 1
fi

if git -C "$kernel_tree" show-ref --verify --quiet \
    refs/heads/reference/vino-production-20260727; then
    diff -u \
        <(
            git -C "$kernel_tree" log -p --author='Lyude Paul' \
                "$kernel_base..reference/vino-production-20260727" \
                | git patch-id --stable \
                | awk '{print $1}' \
                | LC_ALL=C sort
        ) \
        <(
            git -C "$kernel_tree" log -p --author='Lyude Paul' \
                "$kernel_base..$kernel_head" \
                | git patch-id --stable \
                | awk '{print $1}' \
                | LC_ALL=C sort
        )
fi

cargo fmt --manifest-path "$userspace_tree/Cargo.toml" --all -- --check

if [ "${SKIP_BUILD:-0}" = "1" ]; then
    echo "validation: source, history, patch, and formatting checks passed"
    exit 0
fi

make -C "$kernel_tree" LLVM=1 -j"$jobs" \
    rust/kernel.o \
    drivers/gpu/drm/evdi/evdi.o \
    drivers/gpu/drm/vino/vino.o
make -C "$userspace_tree" test
make -C "$userspace_tree" lint
make -C "$userspace_tree" chimera
EVDI_LIB_DIR="$workspace/evdi/library" \
    cargo test --manifest-path "$userspace_tree/Cargo.toml" \
        -p vino-chimera --features live --lib

echo "validation: all build-only checks passed"
