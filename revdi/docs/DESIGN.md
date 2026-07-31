# Vino userspace design

Vino owns the DL3 dock side and reuses upstream EVDI for compositor integration.
There is one display ABI, the stock `evdi_lib.h` ABI.

## Safety boundary

`chimera/src/evdi.rs` mirrors the small set of stock C structures and functions
used by the service. The module owns the device handle, callback state, and
registered framebuffer. It exposes only safe Rust operations:

- owned mode/cursor/DDC-CI events;
- a frame borrow tied to the registered framebuffer;
- checked XRGB8888-to-RGB888 conversion;
- deterministic unregister/disconnect/close in `Drop`.

No raw EVDI handle or callback pointer leaves that module. USB FFI remains
isolated in `vino-driver`.

## Retained Vino sources

Chimera keeps the exact Vino control-plane and WHT codec sources under
`chimera/vino/`. They are not an EVDI reimplementation. `kernel-shim` provides
only the userspace definitions required to compile those sources.

## Build ownership

The repository root registers upstream EVDI as the `evdi/` submodule. `make
chimera` builds `evdi/library` before linking the live service. Offline protocol
and codec tests do not link EVDI or libusb.
