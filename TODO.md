# TODO

- [ ] Add output hotplug support. Both backends currently create exactly one
  output at startup. `OutputAdded` is emitted once; `OutputRemoved` is defined
  in the protocol but never emitted because neither backend tears down or adds
  outputs at runtime.
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
  allocations from the initial footprint. `src/stats.rs` only tracks frames.
- [x] Keep one cancellable idle-dimming timer. Input only advances the idle
  deadline; the timer reschedules itself until that deadline and is removed
  when dimming is disabled or its configuration changes. Event-loop tests
  cover sustained input, cancellation, reconfiguration, and expiry.
- [ ] Lazily initialize the application-icon resolver and retain only the
  desktop-entry fields needed for icons. `BlairState::new` constructs it
  even when no window icons are drawn; `ApplicationIconResolver` in
  `../delib/src/application_icons.rs` eagerly parses and retains every
  installed desktop entry, including localized names and unrelated fields.
- [ ] Add a byte/entry budget and eviction policy to `IconCache` in
  `src/decorations/icons.rs`. Every distinct `(app_id, size)` retains decoded
  pixels or a negative result until the icon theme changes; closing windows
  never evicts entries, and clients can change their app IDs repeatedly.
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
- [ ] Enforce session-lock focus isolation in every focus/grab path.
  `src/state.rs::focus_interactive_layer_surface` calls `keyboard.set_focus`
  directly on layer commits, bypassing the lock check in `set_keyboard_focus`;
  an exclusive shell layer can regain keyboard focus while locked and receive
  keys forwarded by `src/input.rs`. Guard popup grabs in `src/handlers.rs`
  too, and cancel existing normal-client grabs when engaging the lock.
- [ ] Confirm DRM session locks after the locked frame is presented.
  `src/backend/drm.rs::render` calls `confirm_session_lock` immediately after
  `queue_frame`, before `on_vblank`. `ext-session-lock-v1` requires the locked
  frame to have reached every output before sending `locked`; otherwise a
  locker that suspends on confirmation can race with the old visible frame.
- [ ] Hold a logind sleep-delay inhibitor until the locker has confirmed the
  session lock. `monitor_sleep` in the D-Bus integration only queues a command
  on `PrepareForSleep(true)`, and `src/session_lock.rs::prepare_for_sleep`
  merely spawns the locker. Suspend can therefore occur before it connects
  or before a locked frame is presented. Provide this policy independently
  of whether the optional shell D-Bus API is enabled.
- [ ] Route pointer and touch input to session-lock surfaces. The locked
  branch of `src/input.rs::process_input_event` forwards only keyboard events
  and discards everything else, making lock-screen buttons and touch-based
  authentication unusable. Lock-surface hit testing must exclude normal clients.
- [ ] Apply the legacy D-Bus layout/window setters through the validated
  configuration transaction in `src/integrations.rs`. They mutate
  `self.config` before saving without rebuilding `decoration_theme`, refreshing
  decoration negotiation, or consistently relaying out workspaces. The file
  watcher then skips the identical configuration, leaving live behavior stale;
  failed saves also leave partially changed in-memory settings.
- [ ] Reject invalid XKB layouts transactionally in `src/state.rs::apply_config`.
  `apply_keyboard_config` only logs a failed `set_xkb_config` after replacing
  `self.config`, so callers report success and may persist a layout that was
  never applied. Compile/apply the keymap before committing configuration and
  restore the previous state on failure.
- [ ] Include font identity/generation in the title bitmap cache key in
  `src/render/decoration.rs::update_title`. `reload_system_theme` can replace
  the preferred font without clearing decorations, while the key only tracks
  text, color, dimensions, alignment, and scale; unchanged titles keep glyphs
  from the previous font until another cache input changes.
- [ ] Request redraws when blur state is committed or unset in `src/blur.rs`.
  Both protocol handlers update surface state without notifying the repaint
  scheduler, so a stationary surface can keep its previous blur until an
  unrelated input event or surface commit triggers rendering.
- [ ] Schedule throttled frame callbacks for hidden surfaces independently
  of output damage. `HIDDEN_FRAME_THROTTLE` in `src/render/mod.rs` only limits
  callbacks when `send_frame_callbacks` is already called during a repaint;
  it does not schedule one. Hidden clients can wait indefinitely when the
  visible workspace is idle despite the intended roughly one-second cadence.
