mod auth;
mod backend;
mod command;
mod interface;
mod server;
mod wire;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;

pub use auth::ScreencopyAuth;
pub use backend::DbusIntegration;
pub use interface::{CompositorInterface, CompositorProxy};
pub use server::serve;
pub use wire::{DbusDisplay, DbusInputCapabilities, DbusWindow, DbusWorkspace};

pub const SERVICE_NAME: &str = "org.blair.Compositor";
pub const OBJECT_PATH: &str = "/org/blair/Compositor";
pub const INTERFACE_NAME: &str = "org.blair.Compositor1";
