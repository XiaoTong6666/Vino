# Vino upstream workspace

This repository is the review and reproducibility workspace for Vino, a Rust
DRM/KMS driver for DisplayLink DL3 USB display devices. It also carries Chimera,
the open userspace EVDI-to-Vino service, and pins upstream EVDI as a submodule.

The production source lives in two independent working repositories:

- an external kernel working tree for the upstream series;
- `revdi/` — the standalone Chimera and focused Vino USB workspace;
- `evdi/` — the pinned upstream EVDI module and C library.

Those large working trees are intentionally not embedded in this Git history.
The complete, mail-ready changes are regenerated into `patches/`, and the exact
source revisions are recorded below. This keeps the repository small while the
local checkout still contains both complete projects.

## Source authority and pins

The original implementation was pinned from the `vino` branch in
`../drm-v3`; that branch remains the historical source of truth. All cleanup
work was performed in `kernel/`; the exact pre-cleanup production snapshot is
preserved at `reference/vino-production-20260727`, while review and subsystem
changes are carried by `series/vino-upstream`.

| Item | Revision |
|---|---|
| `drm/drm-next` checked 2026-07-27 | `ea97ab2759506d9a818ffed1009bde01062b4091` |
| `rust/drm-rust-next` checked 2026-07-27 | `6dcbb4b1320fa91fee349462a52bb69135f2e45e` |
| integration base (merge of the two tips) | `90e13d487b3b828669dab730cfdf72d417825869` |
| historical `drm-v3` Vino pin | `19a91f95f357` |
| Lyude Paul's `rvkms-slim` tip checked 2026-07-27 | `25bc8cc7e97fd292bea4b77354aaac7eba6c5385` |
| kernel review branch | `series/vino-upstream` |
| upstream EVDI pin | `2713cd41932f2bd8697953a205862a68a966b5ba` |

The integration base is the current `drm-next` tip with the current
`drm-rust-next` tip merged. This is necessary while the Rust DRM prerequisites
have not reached `drm-next`; it does not replace either upstream history.

## What is here

- [`docs/`](docs/README.md) — curated device, architecture, protocol,
  reverse-engineering, upstream, EVDI/Chimera, and testing documentation;
- [`patches/`](patches/README.md) — historical generated kernel/userspace patch series;
- [`tools/regenerate-patches.sh`](tools/regenerate-patches.sh) — reproducible
  patch export;
- [`tools/check-series.sh`](tools/check-series.sh) — applies the generated
  series in disposable worktrees and compares the result;
- [`tools/validate.sh`](tools/validate.sh) — build-only and source-policy checks.

The older dated notes under `../docs` are preserved as research history. They
are not copied into production source and are not authoritative for the current
driver.

## Quick start

Regenerate the historical kernel patch set:

```sh
KERNEL_TREE=/path/to/vino-kernel ./tools/regenerate-patches.sh
```

Check that they apply and reproduce the source trees:

```sh
KERNEL_TREE=/path/to/vino-kernel ./tools/check-series.sh
```

Run the build-only validation:

```sh
KERNEL_TREE=/path/to/vino-kernel ./tools/validate.sh
```

These scripts do not install modules, install a kernel, change the bootloader,
start or stop services, access the dock, or reboot the machine.

## Validation status

The userspace workspace tests pass, stock EVDI is pinned as a submodule, and
the optimized Chimera service builds against its C library. No live dock,
module-load, kernel-install, or reboot test was performed during this cleanup.
