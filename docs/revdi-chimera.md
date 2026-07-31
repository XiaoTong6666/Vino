# EVDI and Chimera

Chimera replaces the proprietary dock-side manager while retaining the upstream
EVDI display stack.

```text
compositor
   | DRM/KMS
   v
upstream evdi.ko
   | evdi_lib.h C ABI
   v
upstream libevdi.so
   | safe wrapper in chimera/src/evdi.rs
   v
Chimera
   | Vino protocol + WHT codec + libusb
   v
D6000
```

The upstream EVDI repository is pinned as the root `evdi/` submodule. Vino no
longer carries a second EVDI kernel module or replacement C library.

## Ownership

The safe wrapper owns the EVDI device handle, callback state, and registered
framebuffer. It converts callbacks into owned Rust events and exposes frame data
as a borrow tied to the device. Raw pointers are confined to that module.

Chimera owns the complementary dock-side path:

1. USB initialization and encrypted control session;
2. shared downstream authentication and EDID discovery;
3. EVDI output connection and mode selection;
4. XRGB8888 conversion and Vino WHT encoding;
5. encrypted mode/control messages and USB video submission;
6. DPMS, cursor, DDC/CI, topology, and session recovery.

## Build

```sh
git submodule update --init evdi
make -C revdi chimera
make -C revdi test
```

The build uses `evdi/library` from the submodule. Offline tests do not require a
loaded module or attached dock.

The bounded `chimera-shared-ake-probe` binary stops after the shared AKE / SKE /
repeater closure. On `17e9:433f` it is now expected to report success before any
per-head authentication or mode ownership begins.
