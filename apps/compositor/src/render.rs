use blair_protocol::Rect;
use smithay::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements, Kind,
        },
        gles::{ffi, GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform},
        utils::draw_render_elements,
        Color32F, Frame,
    },
    desktop::{layer_map_for_output, PopupManager, Window},
    output::Output,
    utils::{Buffer, Physical, Rectangle, Size},
    wayland::{
        compositor::{with_surface_tree_downward, SurfaceAttributes, TraversalAction},
        seat::WaylandFocus,
        shell::wlr_layer::Layer,
    },
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    config::WindowLayout,
    decorations::{
        compile_rounded_corner_shader, DecorationFrame, DecorationTheme, RoundedCornerShaders,
    },
    state::BlairState,
};

pub const BACKGROUND_COLOR: Color32F = Color32F::new(0.08, 0.08, 0.12, 1.0);

pub fn decoration_theme(state: &BlairState) -> DecorationTheme {
    state.config.decorations.to_theme()
}

pub fn ensure_rounded_corner_shader(
    renderer: &mut GlesRenderer,
    cache: &mut Option<RoundedCornerShaders>,
) -> Option<RoundedCornerShaders> {
    if cache.is_none() {
        match compile_rounded_corner_shader(renderer) {
            Ok(shaders) => *cache = Some(shaders),
            Err(err) => tracing::warn!(%err, "failed to compile rounded corner shaders"),
        }
    }
    cache.clone()
}

pub fn z_ordered_windows(state: &BlairState, output: &Output) -> Vec<Window> {
    let mut windows: Vec<_> = state
        .space
        .elements_for_output(output)
        .filter(|window| state.window_visible_on_output(window, output))
        .cloned()
        .collect();
    windows.sort_by_key(|window| state.window_always_on_top(window));
    windows
}

fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    matches_layer: impl Fn(Layer) -> bool,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let layer_map = layer_map_for_output(output);
    layer_map
        .layers()
        .filter(|surface| matches_layer(surface.layer()))
        .filter_map(|surface| {
            layer_map
                .layer_geometry(surface)
                .map(|geo| (geo.loc, surface))
        })
        .flat_map(|(loc, surface)| {
            surface.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                (loc.x, loc.y).into(),
                1.0.into(),
                1.0,
            )
        })
        .collect()
}

/// Background/bottom layer-shell surfaces (e.g. wallpaper), drawn beneath every window.
pub fn bottom_layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    layer_elements(renderer, output, |layer| {
        matches!(layer, Layer::Background | Layer::Bottom)
    })
}

/// Top/overlay layer-shell surfaces (e.g. a panel), drawn above every window.
pub fn top_layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    layer_elements(renderer, output, |layer| {
        matches!(layer, Layer::Top | Layer::Overlay)
    })
}

/// Render elements for each toplevel window's own surface tree, bottom to
/// top. Popups are intentionally excluded — they are collected separately by
/// [`popup_elements`] so they never get clipped by window rounding.
pub fn window_content_elements(
    renderer: &mut GlesRenderer,
    state: &BlairState,
    output: &Output,
) -> Vec<(Window, Vec<WaylandSurfaceRenderElement<GlesRenderer>>)> {
    z_ordered_windows(state, output)
        .into_iter()
        .filter_map(|window| {
            let loc = state.space.element_location(&window)?;
            let wl_surface = window.wl_surface()?;
            let elements = render_elements_from_surface_tree::<
                GlesRenderer,
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                renderer,
                &wl_surface,
                (loc.x, loc.y),
                1.0,
                state.window_opacity(&window, output),
                Kind::Unspecified,
            );
            Some((window, elements))
        })
        .collect()
}

/// Render elements for every popup, positioned in absolute output space.
/// Popups are never clipped to a window's rounded corners.
pub fn popup_elements(
    renderer: &mut GlesRenderer,
    state: &BlairState,
    output: &Output,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let mut elements = Vec::new();
    for window in z_ordered_windows(state, output) {
        let Some(loc) = state.space.element_location(&window) else {
            continue;
        };
        let Some(wl_surface) = window.wl_surface() else {
            continue;
        };
        let geo_loc = window.geometry().loc;
        for (popup, popup_offset) in PopupManager::popups_for_surface(&wl_surface) {
            let popup_geo_loc = popup.geometry().loc;
            let x = loc.x + geo_loc.x + popup_offset.x - popup_geo_loc.x;
            let y = loc.y + geo_loc.y + popup_offset.y - popup_geo_loc.y;
            elements.extend(render_elements_from_surface_tree::<
                GlesRenderer,
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                renderer,
                popup.wl_surface(),
                (x, y),
                1.0,
                state.window_opacity(&window, output),
                Kind::Unspecified,
            ));
        }
    }
    elements
}

/// The client-supplied drag icon for an active drag-and-drop grab (see
/// [`crate::state::BlairState::dnd_icon`]), positioned at the pointer.
/// Drawn last so it stays above every window and popup, like a cursor.
pub fn dnd_icon_elements(
    renderer: &mut GlesRenderer,
    state: &BlairState,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let Some(icon) = state.dnd_icon.as_ref() else {
        return Vec::new();
    };
    let location = state.seat.get_pointer().map_or((0, 0).into(), |pointer| {
        pointer.current_location().to_i32_round()
    });
    render_elements_from_surface_tree::<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>(
        renderer,
        icon,
        (location.x, location.y),
        1.0,
        1.0,
        Kind::Unspecified,
    )
}

struct WindowFrame {
    client_rect: Rect,
    frame_rect: Rect,
    has_border: bool,
    has_titlebar: bool,
}

fn window_frame(state: &BlairState, window: &Window) -> Option<WindowFrame> {
    let loc = state.space.element_location(window)?;
    let client = window.geometry();
    let client_rect = Rect {
        x: loc.x,
        y: loc.y,
        width: client.size.w,
        height: client.size.h,
    };
    let has_border = state.window_has_server_decoration(window);
    let has_titlebar = has_border && !matches!(state.config.window.layout, WindowLayout::Tiling);
    let frame_rect = if has_border {
        DecorationFrame::compute(client_rect, &decoration_theme(state), has_titlebar).frame
    } else {
        client_rect
    };
    Some(WindowFrame {
        client_rect,
        frame_rect,
        has_border,
        has_titlebar,
    })
}

/// Draws one window's content and, if it wants server-side decoration, its
/// frame — clipped to a rounded rectangle via the stencil buffer so the
/// corners reveal whatever was already drawn (background, layers, windows
/// below) instead of being painted over. The themed border is painted as a
/// rounded ring following the same outer curve, so it never gets clipped off
/// at a sharp angle the way a plain rectangle would.
pub fn draw_window(
    frame: &mut GlesFrame<'_, '_>,
    state: &BlairState,
    window: &Window,
    content: &[WaylandSurfaceRenderElement<GlesRenderer>],
    damage: &[Rectangle<i32, Physical>],
    shaders: Option<&RoundedCornerShaders>,
) -> Result<(), GlesError> {
    let Some(WindowFrame {
        client_rect,
        frame_rect,
        has_border,
        has_titlebar,
    }) = window_frame(state, window)
    else {
        return Ok(());
    };

    let radius = state
        .config
        .decorations
        .corner_radius
        .min(frame_rect.width / 2)
        .min(frame_rect.height / 2);
    let clip = radius > 0 && shaders.is_some();

    if clip {
        begin_rounded_clip(frame, &shaders.unwrap().mask, frame_rect, radius)?;
    }

    draw_render_elements(frame, 1.0, content, damage)?;

    let theme = decoration_theme(state);
    let focused = state.window_id(window) == state.focused_window;
    let border_color = rgba(if focused {
        theme.active_border
    } else {
        theme.inactive_border
    });

    if has_titlebar {
        let geom = DecorationFrame::compute(client_rect, &theme, true);
        let titlebar_color = rgba(if focused {
            theme.active_titlebar
        } else {
            theme.inactive_titlebar
        });
        for (rect, color) in [
            (geom.titlebar, titlebar_color),
            (geom.close_btn, rgba(theme.close_button)),
            (geom.maximize_btn, rgba(theme.maximize_button)),
            (geom.minimize_btn, rgba(theme.minimize_button)),
        ] {
            if rect.width > 0 && rect.height > 0 {
                draw_decoration_rect(frame, damage, rect, color)?;
            }
        }
    }

    if has_border {
        if clip {
            let inner_radius = (radius - theme.border_width).max(0);
            draw_border_ring(
                frame,
                &shaders.unwrap().ring,
                frame_rect,
                radius,
                inner_radius,
                border_color,
            )?;
        } else {
            let geom = DecorationFrame::compute(client_rect, &theme, has_titlebar);
            for rect in [
                geom.border_top,
                geom.border_left,
                geom.border_right,
                geom.border_bottom,
            ] {
                draw_decoration_rect(frame, damage, rect, border_color)?;
            }
        }
    }

    if clip {
        end_rounded_clip(frame)?;
    }

    Ok(())
}

fn draw_border_ring(
    frame: &mut GlesFrame<'_, '_>,
    shader: &GlesPixelProgram,
    frame_rect: Rect,
    radius: i32,
    inner_radius: i32,
    color: Color32F,
) -> Result<(), GlesError> {
    let dest: Rectangle<i32, Physical> = Rectangle::new(
        (frame_rect.x, frame_rect.y).into(),
        (frame_rect.width, frame_rect.height).into(),
    );
    let size: Size<i32, Buffer> = (frame_rect.width, frame_rect.height).into();
    let src = Rectangle::from_size(size).to_f64();
    frame.render_pixel_shader_to(
        shader,
        src,
        dest,
        size,
        None,
        1.0,
        &[
            Uniform::new("radius", radius as f32),
            Uniform::new("inner_radius", inner_radius as f32),
            Uniform::new("color", (color.r(), color.g(), color.b(), color.a())),
        ],
    )
}

fn begin_rounded_clip(
    frame: &mut GlesFrame<'_, '_>,
    shader: &GlesPixelProgram,
    frame_rect: Rect,
    radius: i32,
) -> Result<(), GlesError> {
    frame.with_context(|gl| unsafe {
        gl.Clear(ffi::STENCIL_BUFFER_BIT);
        gl.Enable(ffi::STENCIL_TEST);
        gl.StencilFunc(ffi::ALWAYS, 1, 0xFF);
        gl.StencilOp(ffi::KEEP, ffi::KEEP, ffi::REPLACE);
        gl.ColorMask(ffi::FALSE, ffi::FALSE, ffi::FALSE, ffi::FALSE);
    })?;

    let dest: Rectangle<i32, Physical> = Rectangle::new(
        (frame_rect.x, frame_rect.y).into(),
        (frame_rect.width, frame_rect.height).into(),
    );
    let size: Size<i32, Buffer> = (frame_rect.width, frame_rect.height).into();
    let src = Rectangle::from_size(size).to_f64();
    frame.render_pixel_shader_to(
        shader,
        src,
        dest,
        size,
        None,
        1.0,
        &[Uniform::new("radius", radius as f32)],
    )?;

    frame.with_context(|gl| unsafe {
        gl.ColorMask(ffi::TRUE, ffi::TRUE, ffi::TRUE, ffi::TRUE);
        gl.StencilFunc(ffi::EQUAL, 1, 0xFF);
        gl.StencilOp(ffi::KEEP, ffi::KEEP, ffi::KEEP);
    })
}

fn end_rounded_clip(frame: &mut GlesFrame<'_, '_>) -> Result<(), GlesError> {
    frame.with_context(|gl| unsafe {
        gl.Disable(ffi::STENCIL_TEST);
    })
}

pub fn draw_decoration_rect<F: Frame>(
    frame: &mut F,
    damage: &[Rectangle<i32, Physical>],
    rect: Rect,
    color: Color32F,
) -> Result<(), F::Error> {
    if rect.width <= 0 || rect.height <= 0 {
        return Ok(());
    }
    let dst: Rectangle<i32, Physical> =
        Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into());
    frame.draw_solid(dst, damage, color)
}

pub fn rgba(color: [u8; 4]) -> Color32F {
    Color32F::new(
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0,
    )
}

pub fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

// Popups are separate surface trees and need independent frame callbacks.
pub fn send_frame_callbacks(state: &BlairState, time: u32) {
    for surface in state.xdg_shell_state.toplevel_surfaces() {
        let wl_surface = surface.wl_surface();
        send_frames_surface_tree(wl_surface, time);
        for (popup, _) in PopupManager::popups_for_surface(wl_surface) {
            send_frames_surface_tree(popup.wl_surface(), time);
        }
    }
    for layer in &state.layer_surfaces {
        let wl_surface = layer.wl_surface();
        send_frames_surface_tree(wl_surface, time);
        for (popup, _) in PopupManager::popups_for_surface(wl_surface) {
            send_frames_surface_tree(popup.wl_surface(), time);
        }
    }
}
