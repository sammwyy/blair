use smithay::{
    reexports::wayland_protocols_wlr::layer_shell::v1::server::{
        zwlr_layer_shell_v1::ZwlrLayerShellV1,
        zwlr_layer_surface_v1::{Error, Request, ZwlrLayerSurfaceV1},
    },
    wayland::shell::wlr_layer::{
        WlrLayerShellGlobalData, WlrLayerShellState, WlrLayerSurfaceUserData,
    },
};
use wayland_server::{backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::state::BlairState;

wayland_server::delegate_dispatch!(BlairState: [ZwlrLayerShellV1: ()] => WlrLayerShellState);
wayland_server::delegate_global_dispatch!(BlairState: [ZwlrLayerShellV1: WlrLayerShellGlobalData] => WlrLayerShellState);

impl Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData> for BlairState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwlrLayerSurfaceV1,
        request: Request,
        data: &WlrLayerSurfaceUserData,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        // Smithay converts these unsigned sizes to signed geometry before commit hooks run.
        if let Request::SetSize { width, height } = request {
            if width > i32::MAX as u32 || height > i32::MAX as u32 {
                tracing::debug!(width, height, surface = ?resource.id(), "invalid layer size");
                resource.post_error(
                    Error::InvalidSize,
                    "layer size exceeds the supported coordinate range",
                );
                return;
            }
        }
        <WlrLayerShellState as Dispatch<_, _, Self>>::request(
            state, client, resource, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        resource: &ZwlrLayerSurfaceV1,
        data: &WlrLayerSurfaceUserData,
    ) {
        <WlrLayerShellState as Dispatch<_, _, Self>>::destroyed(state, client, resource, data);
    }
}
