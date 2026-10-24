# libobsbot

A cross-platform Rust SDK for [OBSBOT](https://www.obsbot.com)
cameras. Implements the OBSBOT camera-control wire protocol from observed USB
traffic and ships a stable C ABI so the same library can drive PTZ, image
controls, white balance, HDR, FOV, face AE/focus, AI auto-framing, and audio
controls from any GPL-compatible application.

**Status:** v0.0.0 - pre-release. The Meet 2 protocol is decoded -
including the selector-0x02 RPC CRC - and the synchronous control
surface works end-to-end against real hardware. A per-device status
poller and a hot-plug watcher feed the same event channel; the C ABI
mirrors the Rust surface. Full docs at
<https://ananthb.github.io/libobsbot/>.

## Tiny 3 command line

`obsbot` controls the full OBSBOT Tiny 3 (`3564:ff02`) directly over USB.
It uses `libobsbot-core` and its macOS USB/UVC transport; no OBSBOT Center,
vendor SDK, or background service is needed. Tiny 3 Lite is not supported.
See the [Tiny 3 protocol and hardware notes](doc/protocol/tiny3/README.md)
for the reference revision, connected device identity, zoom conversion,
and validation results.

Build with Rust 1.85 or newer:

```sh
cargo build --release -p obsbot
./target/release/obsbot --help
./target/release/obsbot devices
```

To install into Cargo's executable directory (normally `~/.cargo/bin`):

```sh
cargo install --path crates/obsbot --locked
```

Commands after installation:

```sh
obsbot devices
obsbot track
obsbot track on
obsbot track off
obsbot zoom
obsbot zoom 1.5
obsbot --device usb-1001ad64c track
obsbot --device usb-1001ad64c zoom 1.5
```

Replace `usb-1001ad64c` with an ID printed by `obsbot devices`. IDs identify
physical cameras, deduplicating USB interfaces, and remain valid across
CLI invocations while the camera is connected. Reconnecting can change an ID.
With one supported camera selection is automatic; multiple cameras require
`--device`. Control commands fail if no camera is found. Meet 2 cameras remain
available through the library and appear in `devices`, but the CLI's tracking
and zoom factor controls currently require Tiny 3.

`track` reads the actual person-tracking state before toggling it; `track on`
and `track off` set it explicitly. Successful commands print the confirmed
state. `zoom` prints the current magnification, and `zoom 1.5` requests 1.5×.
The library queries the camera's native range and resolution, rejects invalid
or out-of-range factors, and reports the factor achieved after quantization
and readback. Existing raw UVC zoom methods and the C ABI keep their native
unit semantics.

Tiny 3 zoom is calibrated to **1–4×**: native units map to `1 + 0.03 × native`.
A request for `1.5` normally produces `1.51×`. Tested firmware can settle up to
`0.09×` from the requested native step; the CLI prints stable actual readback
and fails if the result cannot be confirmed. Control writes wake a sleeping
camera; queries leave it asleep. See the protocol record for measurements and
limits. Tiny 3 control is currently enabled on macOS only.

> **Have an OBSBOT camera we don't yet support?** The fastest way to
> add it is a USB packet capture. See the
> [hardware-support matrix](https://ananthb.github.io/libobsbot/hardware.html)
> and [open a new-hardware issue](https://github.com/ananthb/libobsbot/issues/new?template=new-hardware.yml)
> with a `.pcapng` attached - we'll do the protocol-decode work from
> there.

## Platforms

| OS | Notes |
|----|-------|
| Linux | First-class. Goal: work while the camera is also open in Zoom/OBS/Cheese - route through the `uvcvideo` driver rather than detaching it. |
| macOS | Supported. Works while other apps (FaceTime, Zoom, OBS, Photo Booth, …) have the camera open. |
| Windows | The camera must be bound to WinUSB via [Zadig](https://zadig.akeo.ie). Binding WinUSB removes the camera from `usbvideo.sys`, so it becomes unusable in Zoom/OBS until the driver is restored. |

## License

GPL-3.0-only. See `LICENSE`. Note that linking to `liblibobsbot` from a
non-GPL-compatible application is incompatible with this license.
