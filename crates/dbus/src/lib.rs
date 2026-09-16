mod backend;
mod command;
mod interface;
mod server;
mod wire;

pub use backend::{drain, CompositorBackend};
pub use command::Command;
pub use interface::{CompositorInterface, CompositorProxy};
pub use server::serve;
pub use wire::DbusWindow;

pub const SERVICE_NAME: &str = "org.blair.Compositor";
pub const OBJECT_PATH: &str = "/org/blair/Compositor";
pub const INTERFACE_NAME: &str = "org.blair.Compositor1";
