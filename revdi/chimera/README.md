# Chimera

Chimera is the userspace service for a stock EVDI display connected to a
Vino-supported DisplayLink dock. It owns control, authentication, mode
programming, WHT encoding, and USB submission. Display capture stays with the
upstream `evdi.ko` and `libevdi.so` from the repository's `evdi/` submodule.

The supported live control profile is Dell D6000 `17e9:6006`. The ThinkPad USB
3.0 Pro Dock `17e9:433f` now has a hardware-verified shared AKE / SKE /
repeater path and a bounded head-0 per-head authentication / mode / video-arm /
first-frame path over its unified `0x02` transport profile. Full multi-head
ownership, cursor, DDC/CI, and long-running recovery still need dedicated
hardware validation. Support is endpoint/profile-specific rather than
vendor-wide.

The code under `vino/` is retained deliberately: it is the exact open Vino
protocol and codec implementation used by both the live encoder and the offline
oracle. `kernel-shim` is a userspace compatibility crate, not a Linux module.

## Daemon

From `revdi/`:

```sh
make chimera
```

Or build directly after building the EVDI C library:

```sh
make -C ../evdi/library
EVDI_LIB_DIR="$(realpath ../evdi/library)" \
  cargo build --release -p vino-chimera --bin chimera --features live
```

For a local run, make the stock library discoverable:

```sh
LD_LIBRARY_PATH=../evdi/library \
  cargo run --release -p vino-chimera --bin chimera --features live
```

The daemon:

1. opens a stock EVDI card before engaging the dock, avoiding a stale control session during
   libevdi's DRM-master wait;
2. initializes the dock and completes HDCP 2.2 authentication;
3. establishes the encrypted control session and authenticates downstream `head 0`;
4. programs the captured `1920x1080@60` head-0 profile while the video/control endpoint is still
   quiescent;
5. obtains the downstream monitor's EDID directly through the dock's `0x20` status / `0x21`
   fetch transactions;
6. connects EVDI and accepts the matching compositor mode;
7. encodes coherent updates with Vino's WHT codec and submits them to USB;
8. forwards DPMS, cursor, and DDC/CI state; and
9. rebuilds owned session state after transport failures.

Chimera does not install/load modules, manage services, or reboot the machine.

## Offline oracle

```sh
cargo run -p vino-chimera --bin chimera-prove
cargo test --workspace
make lint
```

The oracle covers control sealing, message builders, codec records, mode
profiles, DDC/CI bounds, pixel conversion, cursor repacking, strip-grid padding,
and captured DLM fixtures. Live dock validation remains a separate test matrix.

## Hardware takeover

From `revdi/`:

```sh
./scripts/hardware_test.py --ack-display-disruption
```

For the bounded shared-AKE-only probe:

```sh
./scripts/shared_ake_probe.py --ack-display-disruption
```

For the bounded head-0 authentication probe after shared AKE:

```sh
sudo -v
cargo run --release -p vino-chimera --bin chimera-per-head-probe --features live -- 0
```

On the connected `17e9:433f` dock this now completes and prints a real downstream
`Rrx`, plus the final interactive counter and wire sequence. That proves the
open userspace path reaches the end of the bounded per-head control sequence for
head 0 before any mode ownership or video submission begins.

The next bounded layers are also hardware-validated:

```sh
sudo -v
cargo run --release -p vino-chimera --bin chimera-mode-probe --features live -- 0 1920 1080 60
cargo run --release -p vino-chimera --bin chimera-video-arm-probe --features live -- 0 1920 1080 60
cargo run --release -p vino-chimera --bin chimera-first-frame-probe --features live -- 0 1920 1080 60 202020
```

Observed outputs:

```text
mode-probe head=0 1920x1080@60 padded=1920x1088
video-arm-probe head=0 1920x1080@60 padded=1920x1088
first-frame-probe head=0 1920x1080@60 padded=1920x1088 encoded_bytes=119744
```

That gives a bounded end-to-end head-0 path through shared AKE, per-head
authentication, mode programming, video-arm, and first frame submission.

The current daemon deliberately runs in `head 0 only` mode at `1920x1080@60`.
Opening EVDI before dock engagement and programming the dock before the DRM
hotplug are required ordering constraints on the ThinkPad dock's shared EP
`0x02` control/video transport.

The default path does not send the historical video-arm opening/commit burst: current-firmware
captures go directly from mode-set to the first scanout frame. Set `VINO_VIDEO_ARM_BURST=1` only
for the bounded arm experiment (`chimera-video-arm-probe` always exercises it explicitly).

On the ThinkPad dock, video and control share EP `0x02`. The transport coalesces the encoded byte
stream into 64 KiB URBs and keeps at most three video URBs outstanding, matching the stock DLM
usbmon baseline. Dedicated video endpoints retain their depth-eight submit-ahead window.

EVDI can report a full-screen damage rectangle even when only a small region changed. Chimera keeps
the previous captured RGB frame, compares 64x16 strips inside EVDI's coarse damage, and submits only
pixel-different strips. The controlled 64x64 pattern test reduces to four strips and a 320-byte
payload instead of a full-frame update.

The test stops `displaylink-driver.service`, starts Chimera with per-frame
statistics, checks EVDI mode/keyframe setup, creates a small X11 damage workload,
verifies continuous keyframe/damage submission, and restores the
proprietary service through an EXIT/INT/TERM trap. Service management uses a
cached sudo authorization or the desktop PolicyKit prompt. Use
`--interactive-hotplug` for unplug/replug and optional second-head checks.
