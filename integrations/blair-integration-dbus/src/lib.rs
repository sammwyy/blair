mod backend;
mod command;
mod interface;
mod server;
mod wire;

pub use backend::DbusIntegration;
pub use interface::{CompositorInterface, CompositorProxy};
pub use server::serve;
pub use wire::{DbusWindow, DbusWorkspace};

pub const SERVICE_NAME: &str = "org.blair.Compositor";
pub const OBJECT_PATH: &str = "/org/blair/Compositor";
pub const INTERFACE_NAME: &str = "org.blair.Compositor1";
