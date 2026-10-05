//! `blair_window_integration_unstable_v1`: opt-in compositor integration for
//! client windows. The protocol is intentionally Blair-specific; clients must
//! gracefully fall back when its global is not advertised.

#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#![allow(non_upper_case_globals, non_snake_case, unused_imports)]
#![allow(missing_docs, clippy::all)]

#[cfg(feature = "client")]
pub mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocol/blair-window-integration-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("./protocol/blair-window-integration-v1.xml");
}

#[cfg(feature = "server")]
pub mod server {
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocol/blair-window-integration-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("./protocol/blair-window-integration-v1.xml");
}
