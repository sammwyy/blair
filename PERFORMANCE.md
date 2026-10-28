# Performance measurements

## Cropped offscreen buffers

Backdrop blur allocates only its physical rectangle plus `ceil(4 * radius) + 1`
pixels of filter padding on each side, intersected with the output. The extra
pixel preserves bilinear filtering. Shader taps clamp to edge texel centers,
including at output boundaries. The separable Gaussian filters horizontally
into a second buffer that omits horizontal padding, then vertically during
composition. The source capture is released after the horizontal pass. Paired
bilinear taps cover every pixel of the Gaussian support; coefficient recurrence
avoids an exponential calculation per tap. Region screenshots and screencopy allocate
only their requested physical region. Full-output captures retain full-size buffers.

For a 1920×1080 output, the previous RGBA backdrop buffer required 8,294,400
bytes per blur element. A 200×32 physical titlebar with radius 8 now requires
266×98×4 = 104,272 source bytes and 200×98×4 = 78,400 intermediate bytes when
away from output edges (97.8% fewer simultaneous pixel bytes). Only the 78,400
intermediate bytes remain referenced by the blur element after filtering.
A 320×200 region capture needs 256,000 bytes instead of 8,294,400 (96.9% fewer).
These are texture storage calculations, not measured RSS/PSS or driver residency;
driver padding, allocation reuse, and concurrent buffers affect actual savings.
The change does not reduce the graphics stack's empty-session footprint.

Offscreen EGL regressions compare full and cropped blur pixels across three
scales, three radii, and three output positions, including edges. A protocol
fixture compares captured subregions with the full decorated Wayland scene,
including rounded surface clips, at scales 1, 1.25, and 2.
The Gaussian also matches a dense CPU convolution within two channel values at
radii 1, 4, 16, and 64. Dense filtering removes the previous widely spaced tap
pattern; its sample count grows with radius and should be profiled separately
from the buffer savings.

Scene assembly proceeds from background to overlay. Each surface's backdrop
borrows the already assembled lower scene, including previous blurred surfaces.
The capture pass allocates relocation wrappers holding references rather than
moving or duplicating the render elements. Offscreen clip coordinates are reset
before those elements are reused. Window/decorations and surface trees are
built once per scene instead of once for every blur region above them; nested
blur does not recursively reconstruct the lower window stack. Each blur element
still retains its own filtered texture for the frame. This removes repeated
scene allocations, but its RSS/PSS impact has not been measured separately.

## Screencopy framebuffer reuse

Winit reads compatible captures from the composed host back buffer before swap.
DRM reads the swapchain buffer after its render fence when no overlay planes
are active and cursor inclusion matches. The readback covers only the requested
region. Its mapping copies directly to client SHM without an intermediate
`Vec<u8>`; row order, rotation, mirroring, destination stride, and pool offset
are preserved. Output transforms normalize to canonical pixels, so capture flags
remain uninverted. GPU tests verify all eight output transforms and cropped
readbacks; a Wayland protocol test checks SHM pixels, padding, completion,
damage, and fallback admission.
Nested Winit smoke checks with `grim`, `grim -g`, and `grim -c` passed with
framebuffer-reuse trace events. The host surface is made current again after
mapping readback, before swap; this restores EGL state without redrawing the
scene. A following cursor-free capture matched the initial PNG byte for byte.

This path avoids one offscreen RGBA scene texture and one region-sized pixel
vector per compatible capture. For full-output 1920×1080 capture this removes
16,588,800 bytes of pixel storage, excluding the readback PBO and client SHM
which are still required. These are allocation sizes, not measured RSS/PSS.
Cursor mismatches, hardware overlay planes, direct scanout, failed readback,
and requests handled before a scheduled repaint retain the offscreen fallback.
DRM hardware capture behavior and actual recording memory still need measurement.

## Empty nested session

Build the release compositor and run the sampler from an existing graphical
session on Linux. Python 3 and `wayland-info` are required:

```bash
cargo build --release
python3 scripts/measure-nested-memory.py
```

The script prints its artifact directory's `measurements.json` path. Use
`--output <new-directory>` to select the directory or `--binary <release-binary>`
to select an alternative build. It starts a temporary Winit session, samples at
1, 5, and 15 seconds after process launch, and terminates the session afterward.
Keep the host window at its initial size and avoid interacting with it during
sampling. Stop CPU-intensive background work before comparing runs.

Artifacts include the temporary configuration, compositor log, `wayland-info`
output, and JSON with cumulative user/system CPU time, all `smaps_rollup`
metrics, and raw DRM fdinfo counters. The JSON records the checkout commit,
binary SHA-256, and kernel version. Build the selected binary from that checkout
before sampling. Configuration disables autostarts, D-Bus, hot reload, and
idle dimming, and sets output scale to 1. Other system configuration layers
remain effective; the JSON reports whether `/etc/blair` exists.

`ready_seconds` measures observation of the event-loop startup log with a
50 ms polling interval; it does not measure first presentation. CPU time uses
`/proc/<pid>/stat` ticks. RSS includes shared resident pages, while PSS divides
them among the processes mapping them. DRM counters can describe memory already
mapped into the process: report them separately from RSS/PSS. Multiple file
descriptors with the same DRM device/client ID describe the same client and
must not be summed.

## Snapshot: 2026-10-27

Source: `3af79bcefb5021cf405146ad123bdc1a8ebed517`, built with
`rustc 1.97.1 (8bab26f4f 2026-07-14)` and the repository's release profile
(thin LTO, one codegen unit). Local dependencies were CreamUI `0083cb7` and
Delib `9ec05ca`; CreamUI's dependency crates were clean.

- Binary SHA-256: `37b58300390e3bdc29e493b3f1ca60d408af8e30143f81460f502f03bb215456`
- Cargo.lock SHA-256: `a7289b922c9a5b12e6298094bd2c82397c4a563603b4a38b88125940ed227031`
- Kernel: `6.19.14-100.fc42.x86_64`; AMDGPU, PCI `0000:03:00.0`.
- Host: KWin Wayland; Blair output: `winit-0`, 1280×800, scale 1, 60 Hz.
- No shell, application windows, or system Blair configuration was loaded.
  `wayland-info` connected once before the first sample.

Two consecutive runs produced the following values at 15 seconds. Within each
run, RSS, PSS, anonymous memory, and cumulative CPU time were unchanged across
all three samples.

| Metric | Run 1 | Run 2 |
| --- | ---: | ---: |
| RSS (KiB) | 90,756 | 90,796 |
| PSS (KiB) | 40,715 | 40,783 |
| Anonymous resident memory (KiB) | 27,816 | 27,816 |
| PSS from file mappings (KiB) | 12,899 | 12,966 |
| Shared clean mappings (KiB) | 52,536 | 52,492 |
| Shared dirty mappings (KiB) | 4 | 4 |
| Swap (KiB) | 0 | 0 |
| Cumulative CPU time (seconds) | 0.13 | 0.13 |
| Event-loop startup observed (seconds) | 0.151 | 0.151 |

AMDGPU reported identical buffer counters in both runs: `drm-total-gtt` 8,208
KiB, `drm-total-vram` 25,504 KiB, `drm-shared-vram` 4,544 KiB,
`drm-memory-gtt` 33,680 KiB, and `drm-memory-vram` 32 KiB. Three descriptors
shared one DRM client ID per process. GPU engine counters also remained
unchanged between samples.

This is an initial empty-session snapshot. It does not establish memory
retention under window or configuration churn, shell-loaded behavior, or the
DRM backend's footprint; those measurements remain tracked in `TODO.md`.

## Empty nested snapshot after blur changes: 2026-10-28

Source: `5dd27d1a32f927ad098dd8ee1d05740e0673d234`, built with Rust
`1.97.1 (8bab26f4f 2026-07-14)` and the repository's release profile.
Local dependencies were CreamUI `4888d05c451676c1031efff84896ba25face4d8d`
and Delib `6bd877c397ce18c8711df0143df1787b017595df`; all three checkouts
were clean. Both runs used `python3 scripts/measure-nested-memory.py`.

- Binary SHA-256: `a10dca5c3ebaea2c5ccc02c642100684c6a520156bd7e501c2fdf2b0a6f77f3e`
- Cargo.lock SHA-256: `806b07fddf019a8d0560695fdccce69bf3374959a2ab936d9d1304c44ddb6c9d`
- Kernel: `6.19.14-100.fc42.x86_64`; AMDGPU, PCI `0000:03:00.0`.
- Host: KWin Wayland; Blair output: 1280×800, scale 1, 60 Hz.
- No shell, application windows, D-Bus integration, or system Blair configuration.

| Metric | Run 1 | Run 2 |
| --- | ---: | ---: |
| RSS (KiB) | 91,672 | 91,676 |
| PSS (KiB) | 50,562 | 50,576 |
| Anonymous resident memory (KiB) | 27,840 | 27,924 |
| PSS from file mappings (KiB) | 22,721 | 22,652 |
| Shared clean mappings (KiB) | 52,772 | 52,804 |
| Shared dirty mappings (KiB) | 4 | 4 |
| Swap (KiB) | 0 | 0 |
| Cumulative CPU time (seconds) | 0.12 | 0.13 |
| Event-loop startup observed (seconds) | 0.151 | 0.152 |

Each run's metrics and engine counters were unchanged at 1, 5, and 15 seconds.
AMDGPU reported `drm-total-gtt` 8,208 KiB in both runs. Run 1 reported
`drm-total-vram` 25,504 KiB, `drm-shared-vram` 4,544 KiB,
`drm-resident-gtt` 33,680 KiB, and `drm-resident-vram` 32 KiB. Run 2 reported
30,052 KiB, 9,088 KiB, 38,224 KiB, and 36 KiB respectively. Each process had
three descriptors sharing one DRM client ID; counters are reported once per client.

This snapshot does not demonstrate an empty-session memory reduction. Relative
to the earlier snapshot, anonymous memory is within 108 KiB and RSS is about
0.9 MiB higher; PSS is about 9.6 MiB higher, in the file-mapping component.
PSS depends on mapping ownership in other processes. These snapshots also use
different sources and dependencies, so they do not isolate the cause of that
difference. There are no blur regions in this workload; loaded-shell and window
churn measurements are still needed to quantify the offscreen-buffer savings.

## Font fallback retention: 2026-10-28

Run the font-only probe from the adjacent CreamUI checkout:

```bash
cargo run --release -p creamui-fonts --example fallback_memory
```

The same probe was run before and after the bounded fallback-cache change,
using Rust `1.97.1 (8bab26f4f 2026-07-14)` and CreamUI's release profile
(fat LTO, one codegen unit, stripped binaries). Baseline font sources were
CreamUI `0083cb7f8f56ca5b42bebc5f5ceb60b76c5a444b`; updated sources and the
probe are in CreamUI `ef8e836`. The probe's SHA-256 is
`ec50b215636a94e3bade356a5c10b2054a13dd87b5bc55120aa9be1e9a49fb0c`.
Kernel: `6.19.14-100.fc42.x86_64`. No compositor or graphics seat is started.
`creamui-widgets` is outside this probe's dependency graph.

The probe loads the default UI face, shapes U+10FFFF, repeats that miss 100
times, shapes the 32 noncharacters U+FDD0–U+FDEF, then shapes Latin/Hebrew,
Chinese, emoji, and a combining accent. Each returned layout is dropped before
sampling `/proc/self/smaps`. These numbers describe font-file mappings only;
they exclude anonymous cache storage and other process mappings. PSS also
depends on other processes sharing those font pages.

| Stage / metric | Before | After, run 1 | After, run 2 |
| --- | ---: | ---: | ---: |
| Primary face: mappings | 1 | 1 | 1 |
| Primary face: mapped size (KiB) | 404 | 404 | 404 |
| After unsupported character: mappings | 411 | 1 | 1 |
| After unsupported character: mapped size (KiB) | 312,512 | 404 | 404 |
| After unsupported character: RSS (KiB) | 37,524 | 128 | 192 |
| After unsupported character: PSS (KiB) | 36,141 | 28 | 41 |
| After mixed scripts: mappings | 411 | 4 | 4 |
| After mixed scripts: RSS (KiB) | 37,836 | 740 | 784 |
| After mixed scripts: PSS (KiB) | 36,389 | 489 | 497 |
| First unsupported-character search (µs) | 10,857 | 41,469 | 42,282 |
| 100 repeated misses (µs) | 173 | 190 | 244 |
| 32 distinct misses (µs) | 5,360 | 4,017 | 4,075 |

The first search now builds compact coverage ranges and is slower in this
sample. Later searches can reject known fonts without remapping their files.
Timing values are observations from these runs, not a controlled CPU benchmark.
The fallback cache retains at most 32 selected files within 32 MiB of font-file
data, 1,024 coverage records within 8 MiB, and 4,096 weak character results.
Registered faces and live layouts retain their own ownership; those references
are outside these cache limits. Blair's title rasterizer drops its layouts
after producing pixels, and an isolated regression test verifies that unsupported
title characters do not add retained font mappings.
