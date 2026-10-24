# OBSBOT Tiny 3: direct USB controls

Implemented and tested on macOS on 2026-10-24. This is the **full Tiny 3**,
not the Lite. Only enumeration, runtime serial, ordinary person tracking,
standby/wake, and calibrated zoom are supported. No OBSBOT Center,
vendor SDK, or background service was used. The CLI uses libobsbot-core's
existing macOS USB/UVC transport.

## Sources and provenance

The requested public reference is
[joshualambert/obsbot-tiny3-linux](https://github.com/joshualambert/obsbot-tiny3-linux),
commit **`d6ffc62ca0aa4e62e4100838a07339136f802b5a`** (2026-09-28).
Read and used these files at that commit:

| Source | Information used |
|---|---|
| [PROTOCOL.md](https://github.com/joshualambert/obsbot-tiny3-linux/blob/d6ffc62ca0aa4e62e4100838a07339136f802b5a/PROTOCOL.md) | Lite identity, UVC topology, selectors, TLV tracking, status offsets, wake command, reply mailbox behavior, and sleep/streaming distinction |
| `src/discover.rs` | Physical-camera discovery and variable video-node identity; its first-device selection was deliberately replaced with explicit ambiguity handling |
| `src/device.rs` | Ordinary tracking category/submode, serial GET command, sleep/wake frames, sequence matching and bounded polling |
| `src/controls.rs` | Standard UVC absolute zoom; its values are raw units, not magnification |
| `src/frame.rs` | 60-byte V3 header, payload lengths, direction, flags and checksum coverage |
| `src/crc.rs` | CRC-16/USB: reflected polynomial A001, initial FFFF, final XOR FFFF |

The reference verifies **Lite `3564:ff04`**, not this camera. Its identity and
protocol claims were hypotheses until independently checked below. Its zoom
code contains **no factor conversion**. The existing library CRC helper has
the same independently published mathematical algorithm and is reused.

Additional public evidence:

- [OBSBOT Tiny 3 series specifications](https://www.obsbot.com/obsbot-tiny-3-series-4k-ptz-webcam/specs), accessed 2026-10-24: full Tiny 3 digital zoom **1–4×**.
- [lxman/obsbot-mcp calibration](https://github.com/lxman/obsbot-mcp/blob/ec9304b2722745d698db2632eddeacdf24da37cc/docs/superpowers/specs/2026-07-25-zoom-calibration-design.md), commit `ec9304b2722745d698db2632eddeacdf24da37cc`: measured **Tiny 2** law `1 + 0.03 × native`. This supplied a hypothesis and image-registration method, not proof for Tiny 3.
- Apple's installed public `USB.h` / `IOUSBLib.h`: `IOUSBDeviceInterface182`, `IOUSBDevRequestTO`, and the nonexclusive `DeviceRequestTO` ABI. A compiled C layout check verified request size 32 and vtable function offset 240 on this arm64 Mac.

This task explicitly authorizes public protocol facts and direct hardware
observations as evidence instead of requiring new vendor-software captures.
The record below is that evidence; personal camera images are not in the repo.

## Connected camera identity

Actual IOKit USB descriptors (not inferred from the Lite PID):

| Property | Observed value |
|---|---|
| Product | `OBSBOT Tiny 3` |
| Manufacturer | `Remo Tech Co., Ltd.` |
| VID:PID | **`3564:ff02`** |
| USB `bcdDevice` | `0510` (descriptor revision, not a decoded firmware version) |
| `bcdUSB` | `0200` |
| `bDeviceClass/SubClass/Protocol` | `ef/02/01` |
| `iSerialNumber` | `0` — no USB descriptor serial |
| Runtime serial, validated V3 reply | **`RMOWDHHC251RPO`** |
| IOKit registry entry | `0x1001ad64c` |
| USB location / address | `0x00100000` / `1` |
| CLI ID for this connection | **`usb-1001ad64c`** |

Enumeration walks physical USB devices, then deduplicates by registry identity.
Same-model cameras with empty serials remain distinct. IDs survive separate
process invocations while connected; reconnecting may change them. Only
`3564:ff02` is added; Lite/SE are not assumed compatible. Meet 2 discovery and
its MAC-learning handshake remain supported; Tiny 3 skips that handshake.

## Wire format and confirmation

Standard zoom uses CT entity **1**, selector **0x0b**, u16 little endian.
Vendor control uses XU entity **2**, 60-byte transfers:

- Selector **6**: TLV write `16 02 02 00` plus zero padding enables ordinary
  person tracking; `16 02 00 00` disables it.
- Selector 6 GET: marker `2e`; byte **24 / 0x18** reflects the AI category,
  byte **28 / 0x1c** its submode. Confirmed normal states are `(2,0)` and `(0,0)`.
  Category **6** occurred briefly immediately after enabling, followed by 2.
  Its meaning is unknown: the library waits boundedly for a known state and
  never interprets 6 as success. Other unknown values fail.
- Selector 6 byte **2**: asleep 1 / awake 0. Sleeping zoom writes returned USB
  success but left native zoom unchanged. Tracking writes and zoom writes
  therefore wake first, if necessary, and confirm awake before controlling.
  Read-only zoom and serial queries do not wake the camera.
- Power: selector **2**, V3 flags `25`, receiver `02`, command `a0c2`, payload
  `00 00 00 00` wakes; `01 00 00 00` enters standby. These SETs have no reply;
  confirmation uses status byte 2. `standby()` reads without waking, and
  `set_standby(bool)` returns the confirmed state, skipping an unchanged write.
  After a wake transition, the library allows two seconds for firmware startup
  and rechecks awake before returning or sending subsequent controls.
- Serial: selector 2, flags `01`, receiver `0d`, command `18c8`, no payload.
  Reply must have flags `29`, reversed endpoints, the exact sequence and
  command, valid header/payload lengths, and both CRCs. Old mailbox replies
  and echoed requests are rejected.

V3 headers are 12 bytes: `aa flags seq:u16 0c00 crc:u16 host receiver cmd:u16`.
The nested segment is `length:u16 crc:u16 payload`. CRC fields are zeroed for
calculation. Header CRC covers bytes 0–11; nested CRC covers its length, zero
checksum, and payload. The full transfer is always padded to 60 bytes.

A per-device mutex covers whole library transactions. Tiny 3 macOS handles
also hold a nonblocking advisory file lock for their lifetime, so competing
libobsbot processes fail clearly instead of interleaving the mailbox. This
coexists with video capture and does not claim the UVC video interface.
Unrelated controllers do not honor this lock.

Each USB request has a **1-second** timeout. State and reply polling use
**3-second** deadlines with 50 ms sleeps; zoom settling has a **5-second**
deadline. Separate wake/transition phases and wake settling can add these times;
an in-flight
USB request can finish up to one transfer timeout after a poll deadline.
All new reads validate lengths; an accepted SET alone never establishes success.

## Standby, tracking modes, and HDR observations

The full Tiny 3 accepted the reference sleep encoding. Separate processes
confirmed asleep → awake → asleep with independent status reads. An immediate
sleep after the awake flag cleared was ignored and correctly timed out;
allowing two seconds after wake made that transition work. The library now
includes that delay. Tracking remained off, native zoom remained 0 (1×), HDR
flag remained 1, and the idle timer remained 120 seconds. This follow-up task
started and ended asleep; its initial zoom was 1×, unlike the earlier zoom test.

The reference's other tracking modes are group, hand, whiteboard, desk, and
human framing variants upper body, close-up, headless, and lower body. Their
category/submode encodings are documented in the reference `PROTOCOL.md`;
only ordinary person tracking has been verified and exposed for the full Tiny 3.

At this follow-up's start, the full camera's selector-6 status byte **6** was
**1**, which the reference identifies as HDR enabled. This is a settings
readback using the Lite's interpretation; HDR toggling and image comparisons
have not been verified on the full Tiny 3. The HDR setting was left unchanged.

Release CLI checks in separate invocations:

```text
obsbot wake                              → awake
obsbot standby                           → standby
obsbot --device usb-1001ad64c wake        → awake
obsbot --device usb-1001ad64c standby     → standby
obsbot standby                           → standby (already asleep)
obsbot zoom 1.5                          → zoom 1.51×
obsbot zoom 1                            → zoom 1×
obsbot standby                           → standby (restored)
```

**Newly observed limitation:** after standby/wake, `track on` was ignored in
this follow-up, including with five extra seconds of wake settling. Independent
readback remained category 0 and the CLI correctly returned a nonzero timeout.
No active video stream was confirmed in this test; whether a stream is required
after standby is not established. The earlier tracking/reframing tests below
passed. A temporary AVFoundation observation received no frames, so physical
parking and live-video behavior during standby were not independently checked.

Follow-up validation: formatting, workspace tests (76 core, 5 FFI, 4 CLI and
1 existing integration test), strict workspace/all-targets Clippy, and the
release CLI build passed. The C ABI is unchanged. Final independent status
confirmed standby, tracking off, native zoom 0, HDR flag 1, and idle timer 120.

## Zoom calibration and firmware settling

Observed direct requests: **GET_MIN=0, GET_MAX=100, GET_RES=1**.
GET_CUR agreed with selector-6 status bytes 4–5, also u16 little endian.
This numerical range alone does not establish magnification.

Local 1920×1080 AVFoundation captures were compared using SIFT matches and
RANSAC similarity transforms, with stationary background features where
available. Capture and OpenCV measurement tools were temporary validation
utilities, not production dependencies. No image was uploaded.

| Actual native values compared | Measured magnification ratio | Inliers | Median residual |
|---|---:|---:|---:|
| 0 → 17, background | 1.511800 | 390 | 0.520 px |
| 0 → 33, background | 1.991831 | 29 | 0.880 px |
| 0 → 50, stationary backpack | 2.514981 | 23 | 0.751 px |
| 50 → 100, full scene | 1.591659 | 26 | 1.150 px |
| 51 → 100, full scene | 1.579185 | 69 | 0.929 px |
| 76 → 100, full scene | 1.218579 | 107 | 1.261 px |

Independent image chains estimated the full range at **4.002991×** and
**4.003279×**. High-zoom images remove much of the background, so those chain
estimates include moving person/chair features and are approximate. Together
with the lower-zoom background measurements and published endpoints, these
establish the supported conversion:

```text
factor = (100 + 3 × native) / 100
native request = nearest supported step to (factor - 1) / 0.03
```

Thus 1.5× requests native 17 (nominal 1.51×); 2× requests native 33 (1.99×).
Bounds and resolution are queried for every operation, and invalid/nonfinite
factors fail before any wake or control write. The calibrated native scale
is limited to 0–100; other scales require new calibration.

**Firmware limitation:** despite GET_RES=1, the camera sometimes settles
1–3 native units away from the request. Examples: 50→52, 49→47, 75→76.
Repeating writes produced 50→49→51→48 and did not reliably converge.
Intermediate values also change while zoom is moving.

The CLI makes one write, waits for ten equal readbacks (about 0.5 seconds),
and prints the actual achieved factor. It accepts the target, or an achieved
value within three units (0.09×) that is closer to the target than the initial
value. An unchanged, unconfirmed request or a larger miss times out with a
nonzero exit. This measured tolerance is distinct from UVC quantization.
Near-current requests may fail if firmware does not reach the target.

## Final CLI hardware checks

All commands below were independent CLI processes; control readbacks came
from direct USB, not a process cache or the Linux V4L2 control cache.

```text
obsbot devices                         → one Tiny 3, ID and serial above
obsbot track off                       → tracking off
obsbot track                           → tracking on
obsbot --device usb-1001ad64c track      → tracking off
obsbot track on                        → tracking on
obsbot track off                       → tracking off
```

Tracking-on images showed the person centered and the room background moved:
401 background homography inliers, median residual 0.703 px, median displacement
(-284.55, -105.03) px. This confirms observable reframing as well as state changes.

| Requested factor | CLI achieved output | Separate `obsbot zoom` |
|---:|---:|---:|
| 1 | 1× | 1× |
| 1.5, using explicit device ID | 1.51× | 1.51× |
| 2 | 1.99× | 1.99× |
| 2.5 | 2.53× | 2.53× |
| 4 | 4× | 4× |
| 1.5, returning from 4× | 1.48× | 1.48× |

Initial state was tracking **off**, native zoom **17** (**1.51×**).
Final restoration used the release executable. Two near-target writes returned
a timeout (actual native 18, then 16); a third reached **native 17**, printed
`zoom 1.51×`, and an independent query also printed `zoom 1.51×`.
`obsbot track off` then confirmed **tracking off**. The 120-second firmware
sleep timer was never changed.

Validation completed:

- `cargo fmt --all -- --check`
- `cargo test --workspace`: 72 core, 5 FFI, 4 CLI, and 1 existing integration test passed.
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --release -p obsbot`
- `cargo check --workspace --all-targets --target aarch64-unknown-linux-gnu`
- Rustdoc with broken/private intra-doc links denied.
- C header regeneration changed comments only, preserving declarations/layouts.

Private validation image hashes (files are intentionally not committed):

| Artifact | SHA-256 |
|---|---|
| native 0 baseline | `4252c7897f3baa95e7b1f4c9cc98457c4df84ffc384af758edd8e1f10b1eab97` |
| native 17 | `7ec4c6a017c3802aa705b56bb87b693661a9acdec10b34b8bd202b5d034302f0` |
| native 33 | `b714bf8af59474caf11260b4e5b9c13315d28008fcf2676ed0784e50b30aff80` |
| native 100 | `d292977d45ea1275ca4b8b4735cb887441a739b7fa6cb1103132eef631005283` |
| tracking off before | `30c98aac3c5f5ba2113301ea06a792788525f44b1a329785aac70e06c96b8b38` |
| tracking on | `a8f01d2d14cc0904f70311f0f539273cfc11692705fc9b7951b6c023df4531c1` |

## Compatibility and limits

- Tiny 3 controls are enabled on **macOS only**. Linux can enumerate it but
  opening fails explicitly because its standard V4L2 zoom readback may be
  cached. Linux support needs real hardware validation before claiming confirmation.
- Meet 2 behavior and the C ABI are preserved. Existing `zoom` / `set_zoom`
  library methods and C functions retain raw UVC units; new Rust
  `zoom_factor` / `set_zoom_factor` methods perform conversion and confirmation.
- Unverified Meet 2 vendor controls fail on Tiny 3. Tiny 3 does not start the
  Meet 2 status poller or invent a Meet 2 firmware version. Serial works;
  decoding a firmware version is not implemented for this model.
- Physical multi-camera and unplugged scenarios were not available; selection,
  deduplication, missing-device and ambiguity behavior are covered by tests.
- Real hardware was available only for this full Tiny 3; Meet 2 regression
  behavior is covered by existing tests, without a connected Meet 2.
