# TODO

- No output hotplug: both backends create exactly one output at startup.
  `OutputAdded` is emitted once; `OutputRemoved` is defined in the protocol
  but never emitted because neither backend tears down or adds outputs at
  runtime.
- DRM backend drives a single connector (the one with the most reliable
  detected mode); extended/mirrored multi-monitor output is not implemented.
- Shortcuts bound via `BindShortcut` have no owner tracking: if the D-Bus
  peer that registered one disappears without calling `UnbindShortcut`, the
  binding stays active until the compositor restarts.
- Configuration is read once at startup; there is no config reload.
- No damage tracking — every frame redraws the full output.
