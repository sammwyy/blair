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
