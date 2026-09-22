//! `blair_blur_unstable_v1`: blair's own background-blur Wayland protocol.
//!
//! A client asks blair to blur whatever is behind a region of its surface
//! instead of compositing it itself. Shaped like KDE's `org_kde_kwin_blur`
//! (manager binds a per-surface object, `set_region` + `commit`) since
//! that's the natural shape for the problem, but under blair's own
//! namespace: this crate is blair-specific, not a KWin compatibility shim.
//!
//! Enable `client` for the client-side bindings (e.g. from a Wayland
//! backend that wants blair's blur when running under blair), `server` for
//! blair itself.

#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#![allow(non_upper_case_globals, non_snake_case, unused_imports)]
#![allow(missing_docs, clippy::all)]

#[cfg(feature = "client")]
pub mod client {
    //! Client-side API of `blair_blur_unstable_v1`.
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocol/blair-blur-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./protocol/blair-blur-v1.xml");
}

#[cfg(feature = "server")]
pub mod server {
    //! Server-side API of `blair_blur_unstable_v1`.
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocol/blair-blur-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./protocol/blair-blur-v1.xml");
}
