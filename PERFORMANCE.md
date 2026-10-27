# Performance measurements

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
