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
  re-renders the output offscreen for every captured frame instead of reading
  back the frame that was just presented.
- [ ] Add `wlr-output-management-v1`, so monitor layout changes do not require
  editing the configuration and restarting.
- [ ] Add tablet, pen, and on-screen keyboard protocols: `tablet-v2`,
  `text-input-v3`, and `input-method-v2`.
- [ ] Enable per-surface direct scanout of client buffers. The DRM backend
  advertises a single global dmabuf feedback tranche and never sends
  per-surface scanout feedback for fullscreen clients.
- [ ] Rasterize SVG titlebar icons. The icon resolver only handles PNG/JPEG, so
  SVG-only icon themes can leave windows without an icon.
- [ ] Expose decoration button reordering (`decorations.buttons.layout`) in
  Coconut. Its settings UI currently only edits the side and fixed
  minimize/maximize/close order.
- [ ] Optimize `blair_blur_unstable_v1` backdrop capture in `render/blur.rs`:
  render only the cropped blur region, use a two-pass separable Gaussian to
  avoid large-radius tap-pattern aliasing, and support recursively blurred
  backdrops.
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
- [ ] Retain only the desktop-entry fields needed for application icons in
  `../delib/src/application_icons.rs`. Blair now initializes the resolver on
  the first icon lookup, but the resolver still retains every installed
  desktop entry, including localized names and unrelated fields.
- [x] Bound `IconCache` in `src/decorations/icons.rs` to 256 entries and 8 MiB
  of decoded pixels and app-ID storage. Least-recently-used eviction includes
  negative results; oversized entries are not retained, and theme changes
  release all cached entries and reset lazy resolver initialization.
- [ ] Bound font-fallback file retention used by decoration titles.
  `fallback_face` in `../creamui/crates/fonts/src/lib.rs` maps and caches
  every font it tries, including candidates that do not cover the requested
  character. Unsupported title characters can leave the whole installed
  font set mapped; measure resident pages and release unsuccessful candidates.
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
- [ ] Bound integration queues and limit commands processed per event-loop
  iteration. `integrations/blair-integration-dbus/src/backend.rs` uses
  unbounded command/event channels and drains commands until empty;
  `server.rs` adds another unbounded event queue. Slow signal delivery can
  accumulate memory, and sustained requests can starve input and rendering.
  Coalesce replaceable geometry/title events while preserving lifecycle order.
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
- [ ] Schedule throttled frame callbacks for hidden surfaces independently
  of output damage. `HIDDEN_FRAME_THROTTLE` in `src/render/mod.rs` only limits
  callbacks when `send_frame_callbacks` is already called during a repaint;
  it does not schedule one. Hidden clients can wait indefinitely when the
  visible workspace is idle despite the intended roughly one-second cadence.
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
- [ ] Implement the complete layer-surface unmap/remap lifecycle.
  `commit_layer` retains configure state and keeps bufferless surfaces in
  `LayerMap`, which can continue reserving their exclusive zones. Reset the
  protocol state, dismiss non-grabbing child popups, release reserved space,
  and require a new configure/ack cycle before remapping.
- [x] Serialize offscreen EGL tests with a shared `EglTest` guard held through
  resource teardown. All eight tests use the common renderer factory, including
  the multi-context magnifier check. This avoids overlapping driver setup and
  teardown that stalled Mesa's debug callback. The full suite passes with
  parallel execution, as do 25 repeated runs of all eight GPU tests with 16
  test threads. `DEVELOPMENT.md` documents the GPU test command and guard usage.
