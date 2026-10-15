# TODO

- No output hotplug: both backends create exactly one output at startup.
  `OutputAdded` is emitted once; `OutputRemoved` is defined in the protocol
  but never emitted because neither backend tears down or adds outputs at
  runtime.
- DRM backend drives a single connector (the one with the most reliable
  detected mode); extended/mirrored multi-monitor output is not implemented.
- No Xwayland: `class`, `role`, and `type` window rules are parsed but never
  match, and X11-only applications cannot run.
- Tiled windows cannot be rearranged or resized interactively: the layout is
  a master column plus a stack, ordered by window creation.
- Window close and minimize animations are configurable but not implemented;
  only the open and workspace-switch animations run.
- `wlr-screencopy` re-renders the output offscreen for every captured frame
  instead of reading back the frame that was just presented, so screen
  recorders pay for a second render pass.
- No session lock protocol (`ext-session-lock-v1`), so screen lockers cannot
  run.
- No output management protocol (`wlr-output-management-v1`): monitor layout
  changes still require editing the configuration and restarting.
- No tablet, pen, or on-screen keyboard protocols (`tablet-v2`,
  `text-input-v3`, `input-method-v2`).
- Direct scanout of client buffers is not enabled per surface: the DRM
  backend advertises a single global dmabuf feedback tranche and never sends
  per-surface scanout feedback for fullscreen clients.
- The titlebar icon only resolves PNG/JPEG icon files; SVG-only icon themes
  (common for scalable hicolor entries) are not rasterized, so those windows
  render with no icon even though one is installed.
- Title and icon pixels are rasterized at a 1:1 pixel scale rather than the
  output's scale factor, so they read slightly soft on fractional/HiDPI
  outputs.
- The reordering of decoration buttons (`decorations.buttons.layout`) has no
  settings UI in `coconut`; only the button side and the fixed
  minimize/maximize/close order are editable there.
- `blair_blur_unstable_v1`'s backdrop capture (`render/blur.rs`) renders a
  full-output-sized offscreen texture per blurred window per frame instead
  of only the cropped blur region, and its single-pass 9x9-tap shader shows
  faint tap-pattern aliasing at larger radii compared to a real two-pass
  separable Gaussian. Blurred windows also don't recursively blur each
  other's backdrop — a blurred window's backdrop capture renders windows
  behind it without their own blur applied.

## Coconut Settings compatibility

Source review: 2026-10-15. Settings lives in `../coconut/apps/settings`,
with system adapters in `../coconut/integrations/system`. Findings below
come from code inspection, without a running desktop session.

### Missing or incomplete Blair behavior

- [x] Implement focus-follows-mouse and expose its policy to Settings.
  `focus.policy = "sloppy"` focuses hovered windows without interrupting
  pointer or keyboard grabs. Coconut exposes the live policy switch.
  Sources: `src/config.rs` (`FocusPolicy`), `src/input.rs` (`move_pointer`),
  `../coconut/apps/settings/src/views/windows/mod.rs` (`build`).
- [x] Implement configurable hot corners and an API Coconut can use for
  corner actions. `[hot_corners]` supports per-corner workspace navigation,
  show/restore desktop, commands, dwell time, and hit areas through the
  existing configuration API. Settings exposes all controls. Event-loop
  tests verify single activation and cancellation on pointer departure.
  Sources: `src/hot_corners.rs`,
  `../coconut/apps/settings/src/views/desktop/hot_corners.rs`.
- [x] Implement desktop magnification and expose native `zoom` and
  `zoom-factor` settings. The magnifier renders the complete scene to a
  texture, preserving decoration/blur clipping, then enlarges a region
  anchored to the pointer. Surface frame/presentation tracking uses the
  underlying scene's render states. EGL pixel tests verify the enlarged
  output; Coconut exposes the live toggle and factor slider (1–8).
  Sources: `../coconut/apps/settings/src/views/accessibility.rs`,
  `../coconut/integrations/system/src/lib.rs` (`gnome_mapping`),
  `src/config.rs`, `src/render/mod.rs`, `src/render/magnify.rs`.
- [x] Implement sticky modifier keys and expose `sticky-keys` to Settings.
  Modifier taps latch for one chord; double taps lock until a third tap.
  Ordinary held chords retain their behavior. Disabling the preference or
  resetting input releases virtual modifiers. Tests exercise XKB state,
  shortcut modifiers, latching, locking, and configuration changes.
  Sources: `../coconut/apps/settings/src/views/accessibility.rs`,
  `../coconut/integrations/system/src/lib.rs` (`gnome_mapping`), `src/input.rs`.
- [x] Implement visual bell feedback and expose `visual-alerts` to Settings.
  Blair advertises `xdg-system-bell-v1` and flashes a translucent overlay
  for 160 ms when enabled. Requests are rate-limited during an active flash;
  cleanup uses the event loop. Coconut reads/writes the native preference.
  Sources: `../coconut/apps/settings/src/views/accessibility.rs`,
  `../coconut/integrations/system/src/lib.rs` (`gnome_mapping`),
  `src/accessibility.rs`, `src/render/mod.rs`.
- [x] Apply workspace configuration changes at runtime. Count, dynamic
  creation, wrapping, names, and output assignments now update live.
  Reducing a fixed workspace set migrates its windows and active outputs
  into the remaining set, preserves reachable focus, and emits workspace
  events. Configuration readback now contains the applied workspace values.
  Sources: `src/state.rs` (`apply_config`), `src/integrations.rs`
  (`configuration`, `set_configuration`),
  `../coconut/apps/settings/src/views/windows/mod.rs` (`build_working_area`,
  `Writer::save`).
- [x] Complete shortcut registration feedback in Settings. The shared
  `blair_protocol::ShortcutAccelerator` parser now maps Tab, navigation,
  punctuation, function/media keys, and rejects unmapped characters before
  registration. Settings validates with the same parser before saving.
  Bare modifiers activate on release and do not fire during a chord.
  Coconut exposes a live shortcut service, stages registrations on a fresh
  connection, persists successful replacements, and retains the previous
  generation on rejection. Settings displays startup and apply errors.
  An isolated D-Bus test verifies replacement, rejection, persistence,
  dispatch, and release of obsolete event subscriptions.
  Sources: `src/shortcuts.rs`, `../coconut/crates/core/src/shortcuts.rs`,
  `../coconut/apps/settings/src/views/windows/mod.rs` (`ShortcutRecorder`),
  `../coconut/apps/shell/src/shortcuts.rs` (`ShortcutListener::start`).
- [x] Make enabling `general.hot_reload` effective after startup with it
  disabled. The watcher stays installed while reload is disabled and tracks
  ancestor directory creation so newly created config roots are watched.
  Debounced callbacks recheck the flag before applying a queued reload.
  Sources: `src/config.rs` (`watch`), `src/state.rs` (`apply_config`),
  `../coconut/apps/settings/src/views/windows/mod.rs` (`build_general`).
- [x] Expose input-device and backend capabilities to Settings. Blair now
  advertises the backend plus support for every connected device over D-Bus.
  Settings disables unavailable mouse and touchpad controls with an
  explanation, including nested Winit sessions. DRM applies input changes as
  a transaction, restores all devices on failure, and reports native errors
  back to Settings.
  Sources: `src/input_devices.rs`, `src/backend/drm.rs`,
  `crates/protocol/src/input.rs`,
  `../coconut/apps/settings/src/views/windows/mod.rs` (`build_mouse`,
  `build_touchpad`).

The close/minimize animation stubs, missing session-lock protocol, and
X11-only rule selectors are already tracked above. Settings exposes
`animations.window_close.duration` and `animations.minimize.duration`,
although only opening and workspace animations run (`src/state.rs`:
`window_opacity`, `animations_active`, `minimize_window`). Its raw rules
editor also accepts the nonfunctional `class`, `role`, and `type` selectors
(`src/rules.rs`: `CompiledRule::matches`).

### Settings and desktop integration work

- [x] Correct "Center new floating windows" in Coconut. The control now
  edits Blair's live `window.center_new_windows` preference (default true).
  Explicit rule positions still take precedence; disabling centering uses
  the work area's padded upper-left corner.
  Sources: `src/state.rs` (`map_window`),
  `../coconut/apps/settings/src/views/windows/mod.rs` (`build`).
- [x] Connect the cursor-theme picker to Blair's existing `[cursor].theme`
  configuration. Settings applies the Blair preference in the background
  before updating Coconut's shell configuration and reports rejected
  writes. Its selection reflects the native preference when set.
  Sources: `../coconut/apps/settings/src/views/personalization/asset_packs.rs`
  (`cursor_themes`), `src/cursor.rs` (`resolve_theme`, `reconfigure`),
  `src/state.rs` (`apply_config`).
- [x] Synchronize the selected icon theme with Blair's titlebar icons and
  invalidate their cache when it changes. Settings updates Blair's live
  `appearance.icon_theme` and then Coconut's shell resolvers. Unset themes
  still use `delib::icons::system_icon_theme`. A fixture test switches two
  real icon themes and verifies that cached pixels change.
  Sources: `src/decorations/icons.rs`, `../delib/src/icons.rs`,
  `../coconut/apps/settings/src/views/personalization/asset_packs.rs`,
  `../coconut/apps/shell/src/lib.rs` (`RuntimeEvent::ShellConfig`).
- [x] Provide shared accessibility preferences for high contrast (`contrast`)
  and interface text scaling (`text-scale`). CreamUI persists them in
  `cream/accessibility.toml` and applies an opaque contrast palette and
  scaled typography when loading any selected theme. Coconut preserves
  those tokens, updates Settings live, and publishes theme reloads for
  the shell and Blair. Decoration text and titlebar geometry scale too.
  Sources: `../coconut/apps/settings/src/views/accessibility.rs`,
  `../coconut/apps/settings/src/views/personalization/fonts.rs`,
  `../coconut/integrations/system/src/lib.rs` (`discover_preferences`).
- [x] Connect "Require password after sleep" (`lock-after-sleep`) to a
  session locker and suspend policy. Blair implements `ext-session-lock`,
  renders a fail-closed black frame before confirming the lock, and sends
  keyboard input only to the locker. It watches logind's suspend signal and
  launches the configured locker without a shell. Settings enables the
  preference only when it finds an executable `swaylock`, `gtklock`, or
  `waylock`; otherwise it explains why the policy cannot be enabled.
  Sources: `src/session_lock.rs`, `src/handlers.rs`,
  `integrations/blair-integration-dbus/src/server.rs`,
  `../coconut/integrations/system/src/blair.rs`.
- [x] Provide an idle-dimming service and native `idle-dim` preference.
  Blair now schedules dimming from user activity, respects idle inhibitors,
  halves a writable kernel backlight after `power.idle_dim_timeout`, and
  restores the exact saved level on activity or inhibition. Settings writes
  the native policy and disables the control with an explanation when no
  writable kernel backlight is available.
  Sources: `src/idle_dim.rs`, `src/state.rs` (`note_input_activity`),
  `../coconut/integrations/system/src/blair.rs`.
- [x] Connect "Location services" (`location`) to the session's location
  provider/permission policy. Coconut exports a standard per-session
  GeoClue agent; every authorization consults the persisted `[location]`
  policy at request time and denies requests while the setting is off.
  Settings writes that native policy under Blair, rather than GNOME's
  preference, and only enables the control when the administrator has
  explicitly whitelisted `org.coconut.LocationAgent` in GeoClue.
  Sources: `../coconut/apps/shell/src/location.rs`,
  `../coconut/crates/core/src/lib.rs`,
  `../coconut/integrations/system/src/blair.rs`.
- [x] Complete Coconut's shortcut editor. Native capture records Tab,
  function/media keys, and modifier-only chords while temporarily inhibiting
  compositor shortcuts. Escape, focus loss, and navigation cancel capture.
  Shared validation rejects unsupported keys, duplicates, missing commands,
  and oversized sets. Shell edits apply live; a separate editor reads and
  saves Blair's persistent `[[bindings]]` through its configuration API,
  with all four actions and argument validation. Failed persistence leaves
  the compositor's previous configuration intact.
  Sources: `../coconut/apps/settings/src/views/windows/mod.rs`
  (`ShortcutRecorder`, `ShortcutState::save`), `src/integrations.rs`
  (`configured_shortcuts`, `set_configured_shortcuts`).

### Coconut-only gaps and confirmed working paths

- [x] Implement dock auto-hide in Coconut. A two-pixel edge strip reveals
  the dock; leaving schedules a cancellable 350 ms hide timer. Open/pending
  popups keep it visible. CreamUI reports native pointer presence and
  updates layer-shell size/exclusive zones when panels resize.
  Sources: `../coconut/apps/shell/src/auto_hide.rs`,
  `../coconut/apps/settings/src/views/desktop/mod.rs`,
  `../creamui/crates/platform/src/wayland/runtime.rs`.
- [x] Connect Coconut's sound-theme picker to desktop sound playback.
  The shell resolves notification and volume events using the current
  theme, inheritance, and `.disabled` masks, then plays them off the UI
  thread with `pw-play`/`paplay`. Notification sound names and suppression
  hints are honored; tests record requests without producing real audio.
  Source: `../coconut/apps/settings/src/views/personalization/asset_packs.rs`.

Do not list resolution/refresh changes as missing: the DRM backend already
implements mode preview, 15-second automatic rollback, confirmation, and
persistence, and Coconut uses that API. Winit intentionally follows the
host window size. Multi-monitor/hotplug limitations remain tracked above.

"Reduce motion" already maps to Blair's `animations.enabled` when its
configuration API is available. Layout/gaps, floating default size, focus
toggles, decorations, keyboard layout/repeat, and DRM mouse/touchpad
configuration also have implementations; they are not generic stubs.
"Usage analytics" is disabled because Coconut has no telemetry service;
it is not a missing Blair capability. Audio, backlight, networking,
Bluetooth, printers, users, and system time use their own system services;
a missing dependency there does not establish a compositor limitation.
