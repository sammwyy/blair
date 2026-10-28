# TODO

- [ ] Add output hotplug support. Both backends currently create exactly one
  output at startup. `OutputAdded` is emitted once; `OutputRemoved` is defined
  in the protocol but never emitted because neither backend tears down or adds
  outputs at runtime. Newly added outputs must join any pending session-lock
  presentation barrier and display locked content before becoming visible.
- [ ] Support multi-monitor output in the DRM backend. It currently drives a
  single connector (the one with the most reliable detected mode); extended
  and mirrored output are not implemented.
- [ ] Add Xwayland support. `class`, `role`, and `type` window rules are parsed
  but never match, and X11-only applications cannot run.
- [ ] Allow tiled windows to be rearranged and resized interactively. The
  current layout is a master column plus a stack, ordered by window creation.
- [ ] Implement window close and minimize animations. They are configurable,
  but only opening and workspace-switch animations currently run.
- [ ] Avoid a second full render pass for `wlr-screencopy`. It currently
  reads the composed framebuffer directly in Winit and from DRM swapchain
  buffers when no overlay planes are active. Missing cursors are composed from
  a cursor-bounds-only readback, shared by the frame's requests, without desktop
  reconstruction. Requests wait for the scheduled repaint before fallback.
  Mapped readback pixels copy directly into client SHM, with output transforms
  normalized during copying. Removing an already composed cursor, adding a
  magnified cursor, direct scanout, overlay planes, and readback failures use offscreen
  rendering; reuse those paths too. Region fallback captures allocate and render
  only the requested region, preserving rounded clips and decorations.
- [ ] Add `wlr-output-management-v1`, so monitor layout changes do not require
  editing the configuration and restarting.
- [ ] Add tablet, pen, and on-screen keyboard protocols: `tablet-v2`,
  `text-input-v3`, and `input-method-v2`.
- [ ] Enable per-surface direct scanout of client buffers. The DRM backend
  advertises a single global dmabuf feedback tranche and never sends
  per-surface scanout feedback for fullscreen clients.
- [x] Rasterize SVG titlebar icons at the requested physical size. Aspect ratio,
  transparent padding, and straight-alpha cache pixels survive decoration upload;
  embedded images resolve beside the document and system fonts load only for
  SVG text. SVG/PNG/JPEG format selection preserves theme priority. Tests cover
  scaling, transparency, content, cache reuse/invalidation, malformed files,
  raster compatibility, and decode limits. Warm SVG cache hits allocate nothing.
- [ ] Expose decoration button reordering (`decorations.buttons.layout`) in
  Coconut. Its settings UI currently only edits the side and fixed
  minimize/maximize/close order.
- [x] Optimize `blair_blur_unstable_v1` backdrop capture in `render/blur.rs`.
  Captures render only the blur
  rectangle plus filter padding, clamped to the output. EGL tests compare
  cropped and full-output backdrops at integer/fractional scales and output
  edges; region captures also preserve rounded surface clips and decorations.
  A two-pass separable Gaussian uses paired bilinear taps and discards horizontal
  padding after its first pass. GPU tests compare dense CPU convolution at
  radii 1, 4, 16, and 64, including a checkerboard that aliased at large radii.
  Scene assembly proceeds from background to overlay; captures borrow already
  assembled elements, including preceding blur, without rebuilding windows or
  decorations. Protocol/EGL tests cover cumulative blur across bottom layers,
  windows, overlapping top layers, overlays, popup clipping/unset/teardown, and
  the global disable switch.
- [ ] Establish a reproducible release-build memory baseline for DRM and
  Winit, both empty and with the shell loaded. Record RSS/PSS, anonymous
  memory, shared mappings, output resolution/scale, and graphics buffers;
  repeat window open/close and configuration changes to distinguish retained
  allocations from the initial footprint. `PERFORMANCE.md` records the first
  empty Winit samples and `scripts/measure-nested-memory.py` reproduces that
  case; DRM, shell-loaded, and allocation-churn measurements remain pending.
  `src/stats.rs` only tracks frames.
- [x] Keep one cancellable idle-dimming timer. Input only advances the idle
  deadline; the timer reschedules itself until that deadline and is removed
  when dimming is disabled or its configuration changes. Event-loop tests
  cover sustained input, cancellation, reconfiguration, and expiry.
- [x] Retain only desktop IDs and optional icon values in delib's application
  icon resolver. Desktop-entry precedence and app-ID fallbacks remain intact;
  iconless user entries still shadow system entries. A fixture with roughly 1 MiB
  of localized metadata retains less than 128 bytes of ID/icon strings for its
  entries. Caller-selected formats share the resolver's theme ordering.
- [x] Bound `IconCache` in `src/decorations/icons.rs` to 256 entries and 8 MiB
  of decoded pixels and app-ID storage. Least-recently-used eviction includes
  negative results; oversized entries are not retained, and theme changes
  release all cached entries and reset lazy resolver initialization.
- [x] Bound font-fallback retention in CreamUI. Rejected font mappings are
  released; bounded coverage metadata avoids reopening known misses. Selected
  files use a 32-entry/32 MiB cache and character results hold weak references.
  Tests preserve fallback order, normalized clusters, registry invalidation,
  and live face ownership after eviction. A release probe reduced an unsupported
  character's retained font mappings from 411 to 1 and font RSS from 37,524 KiB
  to 128–192 KiB. `PERFORMANCE.md` records the cold-search tradeoff and the
  reproducible probe; Blair has an isolated title-rasterization regression.
- [x] Crop title bitmaps to their painted glyph bounds. Raster scratch holds
  one glyph at a time; retained buffers include only the crop and a transparent
  filtering border. Title sampling keeps the original rectangle so centered
  titles and fractional output scales retain their placement. CPU and EGL
  tests compare the result against full bitmaps and verify width-independent
  storage for short titles.
- [x] Use typed title/button/icon cache keys and borrowed icon lookups.
  Window rendering borrows title/app-ID text and theme snapshots share their
  button layout. Allocation-counting tests verify warm cache hits allocate
  nothing; style, font, size, scale, and icon identity changes invalidate
  buffers without retaining decoded icon pixels through the key.
- [x] Share identical button pixels and renderer textures across windows.
  A bounded weak index releases unused buffers and normalizes invisible
  glyph/color differences. Each button instance keeps its own render ID and
  commit counter; GPU tests cover independent updates, removal, movement,
  opacity, and unchanged frames. Disabled parts release buffers during rendering.
- [x] Reconcile decoration buffers independently of output rendering.
  Configuration/theme changes prune cached windows on every workspace; client
  decoration/hybrid requests, surface commits, fullscreen, and minimization
  prune affected windows. Rendering shares the same part visibility rules.
  Tests cover allocation-free unchanged cleanup, preservation after failed
  saves, hybrid unset negotiation/redraws, and release of sampled GPU textures.
- [x] Reuse magnifier resources per output. The retained texture, damage
  tracker, and element ID preserve scene damage and skip unchanged frames
  without an explicit CPU wait. Mode, scale, and renderer changes replace
  the cache; disabling zoom or removing an output releases it. EGL tests
  verify partial updates, panning, clearing, reuse, and invalidation.
- [x] Cancel obsolete repaint/watchdog timers in `FramePacer`. Presentation,
  forced redraws, and mode changes remove pending wakeups; earlier deadlines
  replace old timers. Event-loop tests also cover missing presentation feedback
  after an animation wakeup.
- [x] Bound D-Bus command admission and processing. The queue retains at most
  128 requests and 8 MiB, with a 1 MiB per-request limit. Batches yield after
  16 commands or a 2 ms time slice and wake the next iteration; one command's
  execution cannot be preempted. Method dispatch does not spawn unlimited
  waiting tasks. Overload and shutdown produce explicit errors; internal
  cleanup/theme/sleep commands await capacity. Private-bus and event-loop tests
  cover errors, memory release, FIFO ordering, backpressure, and fair wakeups.
- [x] Bound integration event queues to 512 events and 8 MiB of accounted
  storage, with a 1 MiB event limit and one signal in flight. Shared queues
  coalesce title, app-ID, geometry, and work-area updates within lifecycle
  barriers. Overflow releases the backlog and emits `EventsReset` before new
  events resume. The blocking bridge is removed; service monitors are cancelled
  on shutdown. Tests cover stalled emission, real signals, byte/count limits,
  lifecycle ordering, and allocation-free warm geometry coalescing.
- [x] Bound client event forwarding with the shared queue. `next_result()`
  reports local overflow, server reset, and service disconnection; legacy
  `next()` ends and exposes `error()`. Subscriptions follow one authenticated
  service owner and release forwarding tasks on consumer drop. Private-bus
  tests cover overflow, resets, resubscription, owner loss, and forged signals.
- [x] Move PNG compression and filesystem writes off the compositor thread.
  A lazy worker limits outstanding captures to two and reserves at most 128 MiB
  of raw pixels plus paths before GPU readback. Replies follow PNG completion
  and flushing; cancellation, saturation, worker recovery, and output mode
  changes are covered by tests, including an EGL capture-to-file check.
- [x] Enforce session-lock focus isolation. Layer and window focus paths use
  the lock policy; lock engagement dismisses popup grabs, finishes interactive
  grabs, and clears normal pointer/touch focus. Only the accepted locker may
  add lock surfaces. In-process Wayland tests verify key delivery during
  pending/active locks, unlock restoration, rejected lockers, and grab teardown.
- [x] Confirm DRM session locks after successful presentation. Queued frames
  carry the lock identity and output; only `on_vblank` completes their lock
  confirmations after `frame_submitted` succeeds. All active outputs must
  present; duplicate, discarded, and previous-lock frames cannot confirm a
  pending lock. Protocol tests cover these transitions and headless sessions.
- [ ] Confirm nested session locks from host presentation feedback.
  `src/backend/winit.rs` currently acknowledges a locked frame after
  `WinitGraphicsBackend::submit`, which swaps EGL buffers but does not report
  when the host compositor actually presents them. Use host presentation
  feedback and account for hidden/minimized host windows.
- [ ] Hold a logind sleep-delay inhibitor until the locker has confirmed the
  session lock. `monitor_sleep` in the D-Bus integration only queues a command
  on `PrepareForSleep(true)`, and `src/session_lock.rs::prepare_for_sleep`
  merely spawns the locker. Suspend can therefore occur before it connects
  or before a locked frame is presented. Provide this policy independently
  of whether the optional shell D-Bus API is enabled.
- [x] Route pointer and touch input to session-lock surfaces. Shared input
  dispatch uses lock-only hit testing, including regions and subsurfaces;
  output-local coordinates, cursor rendering, keyboard selection, and unlock
  focus restoration are covered by protocol/GPU tests. Completed touch frames
  are cancelled before crossing the lock boundary, and hot corners stay disabled.
- [x] Apply legacy D-Bus layout/window setters through the shared validated
  configuration transaction. Persistence completes before live layout, theme,
  keymap, decoration negotiation, workspace, or shortcut updates; failed saves
  preserve live state, and native input changes retain snapshots for rollback.
  Isolated filesystem/protocol tests cover successful updates, all-workspace
  relayout, invalid input, failed directory creation, and failed final renames.
- [x] Reject invalid XKB layouts transactionally. Keymaps are compiled before
  changing devices or configuration; failed preparation preserves keymaps and
  repeat settings. Repeat-only updates keep sticky modifiers and avoid keymap
  rebuilds. Startup fallback records the effective US layout, NUL bytes are
  rejected before calling XKB, and protocol tests cover client notifications.
- [x] Include font identity and registry generation in the title bitmap cache
  key. Font replacements and fallback-font registrations invalidate unchanged
  titles; CPU-renderer tests verify invalidation and stable buffer reuse.
- [x] Request redraws when blur state is committed or unset. Protocol tests
  verify repaint requests without a surface commit, pending-region isolation,
  and suppression of redundant redraws for unchanged blur state.
- [x] Schedule hidden frame callbacks independently of output damage. One
  cancellable timer serves committed callbacks at roughly one-second intervals
  and stops when none remain. Visible callbacks follow their primary output;
  unchanged scheduling allocates nothing. Protocol and EGL tests cover idle
  delivery without redraws, repeated requests, output routing/removal, popup
  teardown, synchronized subsurfaces, callback ordering/timestamp wrapping,
  and workspace, minimization, and session-lock visibility changes.
- [x] Consolidate window rendering parameters into `WindowRenderContext` and
  titlebar sampling parameters into `TitlebarSampleParams`. Both rendering
  helpers now use explicit context/options types without argument-count lint
  suppressions.
- [x] Validate `xdg_popup.grab` before changing focus or the popup stack.
  A bounded, expiring history records delivered key/button presses and touch
  downs. Requests must match the receiving client, seat, and active parent;
  denied popups are dismissed without replacing existing grabs. Protocol
  tests cover forged, foreign, released, replayed, and cancelled serials,
  nested menus, focus restoration, exclusive layers, and interactive grabs.
  Warm input-history updates allocate nothing.
- [x] Route touch sequences through the active popup grab policy. Outside
  touches dismiss nested menus before reaching another client; implicit
  sequences retain their first recipient and skip repeated scene hit tests.
  Accepted popup grabs cancel foreign touch streams after completed frames;
  requests during explicit touch drags are rejected. Protocol tests cover owner
  windows/popups, dead receivers, background touches, layer focus, cancelled
  contacts, duplicate downs, and authorization of the new recipient.
- [x] Arbitrate popup grabs against mapped keyboard-exclusive layers.
  Overlay layers take precedence over top layers; competing menus are dismissed
  before keyboard focus changes. The selected layer's own popups survive
  commits, while unmapping, destruction, or disabled keyboard interactivity
  releases their grabs and restores focus. Protocol tests cover nested menus,
  same-client unrelated windows, priority changes, actual key delivery, and
  popups dismissed before their initial commit.
- [x] Implement the layer-surface unmap/remap lifecycle. Registered surfaces
  retain a weak output assignment, while only mapped surfaces enter `LayerMap`.
  Unmapping resets role state, releases reserved space and input grabs, and
  dismisses child popups in reverse order. New buffers require a configure
  acknowledgement from the current mapping cycle. Protocol and EGL tests cover
  stale/wrapping serials, pending output changes, work-area events, repeated cycles,
  input cleanup, output assignment, and removal without visual remnants.
- [x] Bound layer-shell layout inputs before Smithay arrangement. Reject sizes
  beyond signed coordinates before Smithay's request-time conversion, constrain
  margins and accumulated reservations, and ignore exclusive zones for unsupported
  anchor combinations. Preserve committed requests across output changes without
  applying uncommitted setters. Protocol tests cover all 16 anchor combinations
  with extreme margins and zones, overlapping reservations, resize recovery,
  negative margins, and fractional-scale rounding.
- [x] Serialize offscreen EGL tests with a shared `EglTest` guard held through
  resource teardown. All eight tests use the common renderer factory, including
  the multi-context magnifier check. This avoids overlapping driver setup and
  teardown that stalled Mesa's debug callback. The full suite passes with
  parallel execution, as do 25 repeated runs of all eight GPU tests with 16
  test threads. `DEVELOPMENT.md` documents the GPU test command and guard usage.
