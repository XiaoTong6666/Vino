# Vino userspace

This workspace contains the userspace side of the open DL3 stack:

| Path | Responsibility |
|---|---|
| `chimera/` | Dock control, authentication, mode programming, WHT encoding, EVDI capture, and service lifecycle |
| `vino-driver/` | Focused libusb transport boundary |
| `kernel-shim/` | Minimal userspace compatibility layer required by the vendored Vino protocol and codec sources |

The display side is the upstream DisplayLink EVDI project registered at
`../evdi`. Vino does not carry a second EVDI kernel module or a replacement
`libevdi.so`; Chimera links directly to the stock C ABI.

The USB transport has explicit profiles for the Dell D6000 (`17e9:6006`, split
`0x08/0x0b` video endpoints) and ThinkPad USB 3.0 Pro Dock (`17e9:433f`, both
heads multiplexed over `0x02`). The D6000 control/HDCP profile is the supported
live path. The 433f transport profile is now hardware-verified through shared
AKE / SKE / repeater closure plus a bounded head-0 per-head + mode + video-arm
+ first-frame sequence. Full multi-head ownership remains experimental.
Unknown DisplayLink product IDs are rejected.

## Build

Initialize the EVDI submodule once:

```sh
git submodule update --init evdi
```

Build the production service:

```sh
make chimera
```

That target first builds `../evdi/library/libevdi.so`, then links Chimera against
it. Override the checkout when needed:

```sh
make chimera EVDI_DIR=/path/to/evdi
```

Run the offline suite without hardware:

```sh
make test
make lint
```

`make lint` checks every workspace target with warnings denied. Only the
byte-exact retained files under `chimera/vino/` have local Clippy exclusions.

Run the guarded hardware takeover test from the graphical session:

```sh
./scripts/hardware_test.py --ack-display-disruption
```

Run the bounded shared-AKE-only hardware probe:

```sh
./scripts/shared_ake_probe.py --ack-display-disruption
```

Run the bounded per-head probe for head 0:

```sh
sudo -v
cargo run --release -p vino-chimera --bin chimera-per-head-probe --features live -- 0
```

Current hardware result on `17e9:433f`:

```text
per-head head=0 rrx=e9686c66e60015e4 ack=11 ctr=23 wire=43
mode-probe head=0 1920x1080@60 padded=1920x1088
video-arm-probe head=0 1920x1080@60 padded=1920x1088
first-frame-probe head=0 1920x1080@60 padded=1920x1088 encoded_bytes=119744
```

Add `--interactive-hotplug` to validate physical unplug/replug and a second
monitor. The script records service, USB, XRandR, build, and daemon logs under
`/tmp/vino-hardware-*`, and always restores the proprietary service state on
exit. It uses a cached sudo authorization or the desktop PolicyKit prompt.

## Architecture

```text
compositor
   | DRM/KMS
   v
stock evdi.ko
   | stock libevdi C ABI
   v
Chimera safe Rust wrapper
   | Vino control + codec + libusb
   v
DL3 dock
```

All raw EVDI pointers and callbacks are contained in `chimera/src/evdi.rs`.
Higher service code owns Rust values and borrowed frame slices. The other
intentional unsafe boundary is the libusb transport in `vino-driver`.

The sources under `chimera/vino/` are retained because they are the exact open
protocol and WHT codec implementation used by Chimera's offline oracle and live
encoder. `kernel-shim` only supplies the small kernel prelude expected by those
sources; it is not a kernel module.

## Runtime

The EVDI kernel module must already be loaded, and the proprietary
DisplayLinkManager must not own the dock. The dynamic loader must be able to find
the EVDI library built from the submodule:

```sh
LD_LIBRARY_PATH=../evdi/library \
  cargo run --release -p vino-chimera --bin chimera --features live
```

Chimera never installs or loads kernel modules, changes boot configuration, or
starts/stops the proprietary service.

## License

Vino userspace components are GPL-2.0-or-later. The upstream EVDI submodule
retains its own GPL/LGPL component licenses.
