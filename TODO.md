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
- [ ] Crop title bitmaps to their painted glyph bounds and share identical
  button buffers across windows. `paint_glyphs` in `src/decorations/text.rs`
  allocates the full available title rectangle even for a short title;
  `WindowDecoration` retains separate CPU buffers and renderer textures per
  window. The title allocation grows with the square of the buffer scale.
- [ ] Reuse the magnifier texture, damage tracker, and render-element ID in
  `src/render/magnify.rs`. Each zoomed repaint allocates an output-sized RGBA
  texture, rebuilds the tracker with buffer age zero, and waits synchronously
  for the GPU, forcing a full scene render and new allocation on every frame.
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
- [ ] Move PNG compression and filesystem writes off the compositor thread
  using a bounded worker queue. `src/backend/mod.rs::process_screenshots`
  calls `src/render/capture.rs::screenshot` synchronously, so large captures
  or slow storage block input, client dispatch, and presentation until the
  entire file is written.
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
- [ ] Apply the legacy D-Bus layout/window setters through the validated
  configuration transaction in `src/integrations.rs`. They mutate
  `self.config` before saving without rebuilding `decoration_theme`, refreshing
  decoration negotiation, or consistently relaying out workspaces. The file
  watcher then skips the identical configuration, leaving live behavior stale;
  failed saves also leave partially changed in-memory settings.
- [x] Reject invalid XKB layouts transactionally. Keymaps are compiled before
  changing devices or configuration; failed preparation preserves keymaps and
  repeat settings. Repeat-only updates keep sticky modifiers and avoid keymap
  rebuilds. Startup fallback records the effective US layout, NUL bytes are
  rejected before calling XKB, and protocol tests cover client notifications.
- [ ] Include font identity/generation in the title bitmap cache key in
  `src/render/decoration.rs::update_title`. `reload_system_theme` can replace
  the preferred font without clearing decorations, while the key only tracks
  text, color, dimensions, alignment, and scale; unchanged titles keep glyphs
  from the previous font until another cache input changes.
- [x] Request redraws when blur state is committed or unset. Protocol tests
  verify repaint requests without a surface commit, pending-region isolation,
  and suppression of redundant redraws for unchanged blur state.
- [ ] Schedule throttled frame callbacks for hidden surfaces independently
  of output damage. `HIDDEN_FRAME_THROTTLE` in `src/render/mod.rs` only limits
  callbacks when `send_frame_callbacks` is already called during a repaint;
  it does not schedule one. Hidden clients can wait indefinitely when the
  visible workspace is idle despite the intended roughly one-second cadence.
- [ ] Consolidate rendering parameters in `src/render/mod.rs` for
  `window_elements` and `titlebar_pixel_source`. Both exceed Clippy's argument
  count threshold; use render context/options types when refactoring the pass.
- [ ] Validate initial `xdg_popup.grab` requests against an input serial from
  the focused client. `src/handlers.rs` checks existing grab serials, but with
  no active grab it accepts arbitrary serials and can focus an unfocused app's
  popup. Cover pointer- and keyboard-triggered popups and nested grabs.
