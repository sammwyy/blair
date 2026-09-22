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
