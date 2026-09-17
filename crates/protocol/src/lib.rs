mod event;
mod geometry;
mod window;
mod workspace;

pub use event::CompositorEvent;
pub use geometry::{Point, Rect};
pub use window::{WindowId, WindowInfo};
pub use workspace::WorkspaceInfo;
