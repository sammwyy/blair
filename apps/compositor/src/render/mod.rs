mod blur;
mod capture;
mod clip;
mod decoration;

use std::time::Duration;

use smithay::{
    backend::renderer::{
        element::{
            default_primary_scanout_output_compare,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            render_elements,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            Element, Kind, RenderElementStates,
        },
        gles::{GlesPixelProgram, GlesRenderer, GlesTexProgram},
        Color32F,
    },
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            gles::{GlesTarget, GlesTexture},
            Bind, ExportMem, Offscreen,
        },
    },
    desktop::{
        layer_map_for_output,
        utils::{
            send_frames_surface_tree, surface_presentation_feedback_flags_from_states,
            surface_primary_scanout_output, take_presentation_feedback_surface_tree,
            update_surface_primary_scanout_output, with_surfaces_surface_tree,
            OutputPresentationFeedback,
        },
        PopupManager, Window,
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{compositor::with_states, seat::WaylandFocus, shell::wlr_layer::Layer},
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    config::{DecorationButton, TitlebarColorMode, WindowLayout},
    decorations::DecorationFrame,
    state::BlairState,
};
use blair_protocol::Rect;

pub use capture::{capture_region, screenshot};
pub use clip::RoundedClip;
pub use decoration::WindowDecoration;

use clip::ClippedSurfaceElement;
use decoration::{
    contrasting, mix, ButtonGlyph, ButtonLook, FrameElement, FrameParams, TitlebarPixelSource,
    TitlebarStrip,
};

pub const CLEAR_COLOR: Color32F = Color32F::new(0.08, 0.08, 0.12, 1.0);

/// Who draws the pointer cursor. The nested backend lets the host draw
/// named cursors, so only client-provided cursor surfaces are composited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Composited,
    HostNamed,
    Hidden,
}

/// Frame callbacks of surfaces that are not visible anywhere are still sent
/// at roughly this interval so clients never block forever on them.
const HIDDEN_FRAME_THROTTLE: Duration = Duration::from_millis(995);

render_elements! {
    pub OutputRenderElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Clipped=ClippedSurfaceElement,
    Frame=FrameElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Blur=blur::BlurredBackdropElement,
}

#[derive(Clone)]
pub struct Shaders {
    clip: GlesTexProgram,
    frame: GlesPixelProgram,
    titlebar_blend: GlesTexProgram,
    blur: GlesTexProgram,
}

impl Shaders {
    pub fn compile(renderer: &mut GlesRenderer) -> Option<Self> {
        let shaders = clip::compile(renderer).and_then(|clip| {
            Ok((
                clip,
                decoration::compile(renderer)?,
                decoration::compile_titlebar_blend(renderer)?,
                blur::compile(renderer)?,
            ))
        });
        match shaders {
            Ok((clip, frame, titlebar_blend, blur)) => Some(Self {
                clip,
                frame,
                titlebar_blend,
                blur,
            }),
            Err(error) => {
                tracing::error!(%error, "failed to compile decoration/blur shaders; server-side decorations and background blur disabled");
                None
            }
        }
    }
}

pub struct WindowFrame {
    pub client: Rectangle<i32, Logical>,
    pub frame: Rectangle<i32, Logical>,
    pub has_border: bool,
    pub has_titlebar: bool,
}

pub fn window_frame(state: &BlairState, window: &Window) -> Option<WindowFrame> {
    let client = state.space.element_geometry(window)?;
    let has_border = state.window_has_server_decoration(window);
    let has_titlebar = has_border
        && state.config.window.layout != WindowLayout::Tiling
        && !state.window_is_fullscreen(window);
    let frame = if has_border {
        let geometry =
            DecorationFrame::compute(to_rect(client), state.decoration_theme(), has_titlebar);
        from_rect(geometry.frame)
    } else {
        client
    };
    Some(WindowFrame {
        client,
        frame,
        has_border,
        has_titlebar,
    })
}

pub fn z_ordered_windows(state: &BlairState, output: &Output) -> Vec<Window> {
    let mut windows: Vec<_> = state
        .space
        .elements()
        .filter(|window| state.window_visible_on_output(window, output))
        .cloned()
        .collect();
    windows.sort_by_key(|window| {
        (
            state.window_is_fullscreen(window),
            state.window_always_on_top(window),
        )
    });
    windows
}

/// Builds the render elements of `output`, front to back.
pub fn output_elements(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    shaders: Option<&Shaders>,
    cursor_mode: CursorMode,
) -> Vec<OutputRenderElement> {
    profiling::scope!("output_elements");
    let Some(output_geo) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    let viewport = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_default();
    let ctx = FrameContext {
        output_geo,
        scale,
        viewport,
    };

    let font = creamui_fonts::resolve(
        &creamui_fonts::preferred_family(),
        creamui_fonts::FontWeight::Regular,
    );

    let mut elements = Vec::new();
    cursor_elements(renderer, state, output, &ctx, cursor_mode, &mut elements);
    if let Some(icon) = state.dnd_icon.as_ref().filter(|icon| icon.alive()) {
        let location = state.pointer_location().to_i32_round::<i32>() - output_geo.loc;
        elements.extend(surface_tree_elements(
            renderer,
            icon,
            location.to_physical_precise_round(scale),
            scale,
            1.0,
            Kind::Unspecified,
        ));
    }

    let windows = z_ordered_windows(state, output);
    let fullscreen = windows
        .last()
        .is_some_and(|window| state.window_is_fullscreen(window));

    layer_elements(
        renderer,
        state,
        output,
        &ctx,
        shaders,
        &[Layer::Overlay],
        &mut elements,
    );
    if !fullscreen {
        layer_elements(
            renderer,
            state,
            output,
            &ctx,
            shaders,
            &[Layer::Top],
            &mut elements,
        );
    }
    for (index, window) in windows.iter().enumerate().rev() {
        window_elements(
            renderer,
            state,
            output,
            window,
            &ctx,
            shaders,
            &font,
            &mut elements,
        );
        push_window_blur(
            renderer,
            state,
            output,
            &ctx,
            shaders,
            &font,
            &windows[..index],
            window,
            &mut elements,
        );
    }
    if fullscreen {
        return elements;
    }
    layer_elements(
        renderer,
        state,
        output,
        &ctx,
        shaders,
        &[Layer::Bottom, Layer::Background],
        &mut elements,
    );
    elements
}

struct FrameContext {
    output_geo: Rectangle<i32, Logical>,
    scale: f64,
    viewport: smithay::utils::Size<i32, Physical>,
}

fn surface_tree_elements(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    location: Point<i32, Physical>,
    scale: f64,
    alpha: f32,
    kind: Kind,
) -> Vec<OutputRenderElement> {
    render_elements_from_surface_tree(renderer, surface, location, scale, alpha, kind)
}

fn cursor_elements(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    ctx: &FrameContext,
    cursor_mode: CursorMode,
    elements: &mut Vec<OutputRenderElement>,
) {
    if cursor_mode == CursorMode::Hidden {
        return;
    }
    let pointer = state.pointer_location();
    if !ctx.output_geo.to_f64().contains(pointer) {
        return;
    }
    let relative = pointer - ctx.output_geo.loc.to_f64();
    let icon = match &state.pointer_cursor() {
        CursorImageStatus::Hidden => return,
        CursorImageStatus::Surface(surface) if surface.alive() => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|data| data.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let location = (relative - hotspot.to_f64())
                .to_physical(ctx.scale)
                .to_i32_round();
            elements.extend(surface_tree_elements(
                renderer,
                surface,
                location,
                ctx.scale,
                1.0,
                Kind::Cursor,
            ));
            return;
        }
        CursorImageStatus::Surface(_) => CursorIcon::Default,
        CursorImageStatus::Named(icon) => *icon,
    };
    if cursor_mode == CursorMode::HostNamed {
        return;
    }
    let buffer_scale = output.current_scale().integer_scale();
    let time = state.clock_now();
    let (buffer, hotspot) = state.cursor.image(icon, buffer_scale, time);
    let location = (relative - hotspot.to_f64())
        .to_physical(ctx.scale)
        .to_i32_round::<i32>()
        .to_f64();
    match MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        location,
        buffer,
        None,
        None,
        None,
        Kind::Cursor,
    ) {
        Ok(element) => elements.push(element.into()),
        Err(error) => tracing::warn!(%error, "failed to import cursor image"),
    }
}

pub fn cursor_is_animated(state: &BlairState, output: &Output) -> bool {
    match &state.pointer_cursor() {
        CursorImageStatus::Named(icon) => state
            .cursor
            .is_animated(*icon, output.current_scale().integer_scale()),
        _ => false,
    }
}

/// Renders each layer's own content plus its popups, popups clipped
/// separately (not via `LayerSurface::render_elements`, which bundles them
/// unclipped) so a dock-anchored panel gets the same corner clip as a
/// toplevel-anchored one.
fn layer_elements(
    renderer: &mut GlesRenderer,
    state: &BlairState,
    output: &Output,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    layers: &[Layer],
    elements: &mut Vec<OutputRenderElement>,
) {
    let scale = ctx.scale;
    let map = layer_map_for_output(output);
    for &layer in layers {
        for surface in map.layers_on(layer).rev() {
            let Some(geometry) = map.layer_geometry(surface) else {
                continue;
            };
            let wl_surface = surface.wl_surface();
            for (popup, popup_offset) in PopupManager::popups_for_surface(wl_surface) {
                push_popup_elements(
                    renderer,
                    state,
                    ctx,
                    shaders,
                    &popup,
                    geometry.loc + popup_offset,
                    1.0,
                    elements,
                );
            }
            elements.extend(surface_tree_elements(
                renderer,
                wl_surface,
                geometry.loc.to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }
    }
}

/// Renders one popup at `root` (its window-geometry origin, output-logical
/// coords), clipped to `state`'s popup corner radius.
#[allow(clippy::too_many_arguments)]
fn push_popup_elements(
    renderer: &mut GlesRenderer,
    state: &BlairState,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    popup: &smithay::desktop::PopupKind,
    root: Point<i32, Logical>,
    alpha: f32,
    elements: &mut Vec<OutputRenderElement>,
) {
    let scale = ctx.scale;
    let content_location = root.to_physical_precise_round(scale);
    let content: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        popup.wl_surface(),
        content_location,
        scale,
        alpha,
        Kind::Unspecified,
    );
    // `popup.geometry()` stays zero for these popups (the client never
    // calls xdg_surface::set_window_geometry), so the clip is sized from
    // the actual rendered content instead.
    let bounds = content
        .iter()
        .map(|element| element.geometry(Scale::from(scale)))
        .reduce(|a, b| a.merge(b));
    let clip = shaders.zip(bounds).map(|(_, bounds)| {
        let logical_size: Size<i32, Logical> = (
            (bounds.size.w as f64 / scale) as i32,
            (bounds.size.h as f64 / scale) as i32,
        )
            .into();
        let radius = state.popup_corner_radius(logical_size) as f32 * scale as f32;
        RoundedClip {
            rect: bounds,
            radii: [radius; 4],
            viewport: ctx.viewport,
        }
    });
    for element in content {
        match (clip, shaders) {
            (Some(clip), Some(shaders)) if clip.affects(element.geometry(Scale::from(scale))) => {
                elements
                    .push(ClippedSurfaceElement::new(element, shaders.clip.clone(), clip).into());
            }
            _ => elements.push(element.into()),
        }
    }
}

/// Pushes a blurred backdrop for `window`, if it has an active
/// `blair_blur_v1` region, sampling `windows_behind` (this output's stack
/// below `window`, back to front) plus the bottom/background layers.
#[allow(clippy::too_many_arguments)]
fn push_window_blur(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    font: &fontdue::Font,
    windows_behind: &[Window],
    window: &Window,
    elements: &mut Vec<OutputRenderElement>,
) {
    if !state.config.blur.enabled {
        return;
    }
    let Some(surface) = window.wl_surface() else {
        return;
    };
    let Some(region) = crate::blur::blur_of(&surface) else {
        return;
    };
    let Some(frame) = window_frame(state, window) else {
        return;
    };
    let content_origin = frame.client.loc - window.geometry().loc - ctx.output_geo.loc;
    let frame_rect = match region.0 {
        None => Rectangle::new(content_origin, frame.client.size),
        Some(rect) => Rectangle::new(content_origin + rect.loc, rect.size),
    };
    blur::push_backdrop(
        renderer,
        state,
        output,
        ctx,
        shaders,
        font,
        windows_behind,
        frame_rect,
        elements,
    );
}

fn window_elements(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    window: &Window,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    font: &fontdue::Font,
    elements: &mut Vec<OutputRenderElement>,
) {
    let (Some(frame), Some(surface), Some(id)) = (
        window_frame(state, window),
        window.wl_surface(),
        state.window_id(window),
    ) else {
        return;
    };
    let alpha = state.window_opacity(window, output);
    let scale = ctx.scale;
    let origin = ctx.output_geo.loc;
    let render_loc = frame.client.loc - window.geometry().loc - origin;

    for (popup, offset) in PopupManager::popups_for_surface(&surface) {
        let root = render_loc + window.geometry().loc + offset;
        push_popup_elements(renderer, state, ctx, shaders, &popup, root, alpha, elements);
    }

    let focused = state.focused_window == Some(id);
    let (title_text, app_id) = state
        .windows
        .get(&id)
        .map(|managed| (managed.title.clone(), managed.app_id.clone()))
        .unwrap_or_default();

    let decorated = frame.has_border && shaders.is_some();
    // Sampled before drawing the titlebar contents: in blend mode their
    // colors follow whatever the client paints at its top edge.
    let titlebar_source = match shaders {
        Some(shaders)
            if decorated
                && frame.has_titlebar
                && state.decoration_theme().titlebar_mode == TitlebarColorMode::Blend =>
        {
            let decoration = state.decorations.entry(id).or_default();
            titlebar_pixel_source(
                renderer,
                decoration,
                &surface,
                window,
                frame.client.size,
                scale,
                shaders,
            )
        }
        _ => {
            if let Some(decoration) = state.decorations.get_mut(&id) {
                decoration.strip = None;
            }
            None
        }
    };

    if decorated && frame.has_titlebar {
        let theme = state.decoration_theme().clone();
        let buffer_scale = (scale.ceil() as i32).max(1);
        let geometry = DecorationFrame::compute(to_rect(frame.client), &theme, true);
        let fill = if focused {
            theme.active_titlebar
        } else {
            theme.inactive_titlebar
        };
        let background = titlebar_source
            .as_ref()
            .and_then(|source| source.average)
            .map_or(fill, |average| over(average, fill));
        let foreground = contrasting(background);
        // Unfocused windows fade their text and glyphs toward the
        // background instead of switching to a separate palette.
        let muted = if focused {
            foreground
        } else {
            mix(foreground, background, 0.45)
        };
        let decoration = state.decorations.entry(id).or_default();
        for (index, (button, rect, glyph, semantic)) in [
            (
                DecorationButton::Close,
                geometry.close_btn,
                ButtonGlyph::Close,
                theme.close_button,
            ),
            (
                DecorationButton::Maximize,
                geometry.maximize_btn,
                ButtonGlyph::Maximize,
                theme.maximize_button,
            ),
            (
                DecorationButton::Minimize,
                geometry.minimize_btn,
                ButtonGlyph::Minimize,
                theme.minimize_button,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if rect.width <= 0 || rect.height <= 0 {
                continue;
            }
            let look = if state.hovered_button == Some((id, button)) {
                ButtonLook {
                    glyph,
                    disc: semantic,
                    glyph_color: contrasting(semantic),
                }
            } else {
                ButtonLook {
                    glyph,
                    disc: with_alpha(foreground, if focused { 0x1c } else { 0x10 }),
                    glyph_color: with_alpha(muted, 0xd8),
                }
            };
            decoration.update_button(index, look, rect.width, buffer_scale);
            if let Some((buffer, size)) = decoration.button_buffer(index) {
                let location = Point::<i32, Logical>::from((rect.x, rect.y)) - origin;
                push_memory_element(renderer, buffer, location, size, scale, alpha, elements);
            }
        }

        let title_size = (theme.titlebar_height as f32 * 0.41).clamp(10.0, 18.0);
        decoration.update_title(
            font,
            &title_text,
            title_size,
            muted,
            geometry.title.width,
            geometry.title.height,
            theme.title_centered,
            buffer_scale,
        );
        if let Some((buffer, size)) = decoration.title_buffer() {
            let location =
                Point::<i32, Logical>::from((geometry.title.x, geometry.title.y)) - origin;
            push_memory_element(renderer, buffer, location, size, scale, alpha, elements);
        }

        if theme.show_icon && geometry.icon.width > 0 {
            let icon = app_id.as_deref().and_then(|app_id| {
                state
                    .icon_cache
                    .get(app_id, geometry.icon.width * buffer_scale)
            });
            let key = format!(
                "{}|{}|{buffer_scale}",
                app_id.as_deref().unwrap_or(""),
                geometry.icon.width
            );
            decoration.update_icon(&key, icon.as_deref(), geometry.icon.width, buffer_scale);
            if let Some((buffer, size)) = decoration.icon_buffer() {
                let location =
                    Point::<i32, Logical>::from((geometry.icon.x, geometry.icon.y)) - origin;
                push_memory_element(renderer, buffer, location, size, scale, alpha, elements);
            }
        }
    }

    let clip = decorated.then(|| {
        let radius = state.corner_radius(frame.frame);
        let inner = (radius - state.decoration_theme().border_width).max(0) as f32 * scale as f32;
        let top = if frame.has_titlebar { 0.0 } else { inner };
        RoundedClip {
            rect: Rectangle::new(frame.client.loc - origin, frame.client.size)
                .to_physical_precise_round(scale),
            radii: [top, top, inner, inner],
            viewport: ctx.viewport,
        }
    });
    let content: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        render_loc.to_physical_precise_round(scale),
        scale,
        alpha,
        Kind::Unspecified,
    );
    for element in content {
        match (clip, shaders) {
            (Some(clip), Some(shaders)) if clip.affects(element.geometry(Scale::from(scale))) => {
                elements
                    .push(ClippedSurfaceElement::new(element, shaders.clip.clone(), clip).into());
            }
            _ => elements.push(element.into()),
        }
    }

    if let (true, Some(shaders)) = (decorated, shaders) {
        let theme = state.decoration_theme();
        let params = FrameParams {
            radius: state.corner_radius(frame.frame) as f32,
            border_width: theme.border_width as f32,
            titlebar_height: if frame.has_titlebar {
                theme.titlebar_height as f32
            } else {
                0.0
            },
            border_color: rgba(if focused {
                theme.active_border
            } else {
                theme.inactive_border
            }),
            // Stays painted under the sampled strip so a translucent first
            // row blends onto the theme color, not onto whatever is behind
            // the window.
            titlebar_color: rgba(if focused {
                theme.active_titlebar
            } else {
                theme.inactive_titlebar
            }),
        };
        let decoration = state.decorations.entry(id).or_default();
        decoration.update(frame.frame, params);
        if titlebar_source
            .as_ref()
            .is_some_and(|source| source.changed)
        {
            decoration.damage_titlebar();
        }
        if let Some(element) = decoration.element(
            &shaders.frame,
            origin,
            scale,
            alpha,
            titlebar_source.as_ref().map(|source| &source.pixels),
        ) {
            elements.push(element.into());
        }
    }
}

/// How far below the top edge of the window content the titlebar color is
/// sampled, in logical pixels. Clients often draw a 1px highlight or border
/// on their very first row, which would otherwise tint the whole titlebar.
const TITLEBAR_SAMPLE_DEPTH: f64 = 2.0;

struct SampledTitlebar {
    pixels: TitlebarPixelSource,
    /// Whether the strip changed since the previous frame.
    changed: bool,
    /// The strip's average color, premultiplied, for picking contrasting
    /// text and button colors.
    average: Option<[u8; 4]>,
}

/// Refreshes the window's cached titlebar strip and returns it.
fn titlebar_pixel_source(
    renderer: &mut GlesRenderer,
    decoration: &mut WindowDecoration,
    surface: &WlSurface,
    window: &Window,
    size: smithay::utils::Size<i32, Logical>,
    scale: f64,
    shaders: &Shaders,
) -> Option<SampledTitlebar> {
    let size = size.to_physical_precise_round(scale);
    if size.is_empty() {
        decoration.strip = None;
        return None;
    }
    // Only one row is rendered. Besides being cheap, a 1px-tall target
    // sidesteps GL's bottom-up framebuffers: in a full-height offscreen
    // render, buffer row 0 is the window's *bottom* row.
    let strip_size = smithay::utils::Size::<i32, Physical>::from((size.w, 1));
    let reusable = decoration
        .strip
        .as_ref()
        .is_some_and(|strip| strip.size == strip_size && strip.scale == scale);
    if !reusable {
        let texture = Offscreen::<GlesTexture>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            strip_size.to_logical(1).to_buffer(1, Transform::Normal),
        )
        .map_err(|error| tracing::warn!(%error, "failed to allocate titlebar pixel source"))
        .ok()?;
        decoration.strip = Some(TitlebarStrip {
            texture,
            tracker: OutputDamageTracker::new(strip_size, scale, Transform::Normal),
            size: strip_size,
            scale,
            rendered: false,
            average: None,
        });
    }
    let strip = decoration.strip.as_mut()?;

    let depth = ((TITLEBAR_SAMPLE_DEPTH * scale).round() as i32).clamp(0, size.h - 1);
    let location = (Point::<i32, Logical>::from((0, 0)) - window.geometry().loc)
        .to_physical_precise_round(scale)
        - Point::<i32, Physical>::from((0, depth));
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            surface,
            location,
            scale,
            1.0,
            Kind::Unspecified,
        );
    let changed = {
        let mut framebuffer = renderer
            .bind(&mut strip.texture)
            .map_err(|error| tracing::warn!(%error, "failed to bind titlebar pixel source"))
            .ok()?;
        let age = usize::from(strip.rendered);
        let result = strip
            .tracker
            .render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                Color32F::new(0.0, 0.0, 0.0, 0.0),
            )
            .map_err(|error| tracing::warn!(%error, "failed to render titlebar pixel source"))
            .ok()?;
        let changed = result.damage.is_some_and(|damage| !damage.is_empty());
        if changed || strip.average.is_none() {
            // A single row is tiny; reading it back only when the client
            // repaints that row keeps the stall off the steady state.
            strip.average = average_row(renderer, &framebuffer, strip_size.w);
        }
        changed
    };
    strip.rendered = true;
    Some(SampledTitlebar {
        pixels: TitlebarPixelSource {
            texture: strip.texture.clone(),
            row: Rectangle::new((0.0, 0.0).into(), (strip_size.w as f64, 1.0).into()),
            transform: Transform::Normal,
            program: shaders.titlebar_blend.clone(),
        },
        changed,
        average: strip.average,
    })
}

fn average_row(
    renderer: &mut GlesRenderer,
    framebuffer: &GlesTarget<'_>,
    width: i32,
) -> Option<[u8; 4]> {
    let mapping = renderer
        .copy_framebuffer(
            framebuffer,
            Rectangle::from_size((width, 1).into()),
            Fourcc::Abgr8888,
        )
        .map_err(|error| tracing::debug!(%error, "failed to read back the titlebar strip"))
        .ok()?;
    let pixels = renderer.map_texture(&mapping).ok()?;
    let count = (pixels.len() / 4).max(1) as u64;
    let mut sum = [0u64; 4];
    for px in pixels.chunks_exact(4) {
        for (total, channel) in sum.iter_mut().zip(px) {
            *total += u64::from(*channel);
        }
    }
    Some(sum.map(|total| (total / count) as u8))
}

/// Composites a premultiplied `top` over an opaque straight `base`.
fn over(top: [u8; 4], base: [u8; 4]) -> [u8; 4] {
    let inverse = 255 - u16::from(top[3]);
    let channel =
        |t: u8, b: u8| (u16::from(t) + (u16::from(b) * inverse + 127) / 255).min(255) as u8;
    [
        channel(top[0], base[0]),
        channel(top[1], base[1]),
        channel(top[2], base[2]),
        255,
    ]
}

fn with_alpha(color: [u8; 4], alpha: u8) -> [u8; 4] {
    [color[0], color[1], color[2], alpha]
}

fn push_memory_element(
    renderer: &mut GlesRenderer,
    buffer: &MemoryRenderBuffer,
    location: Point<i32, Logical>,
    size: smithay::utils::Size<i32, Logical>,
    scale: f64,
    alpha: f32,
    elements: &mut Vec<OutputRenderElement>,
) {
    let physical_location: Point<i32, Physical> = location.to_physical_precise_round(scale);
    let physical_location = physical_location.to_f64();
    match MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        physical_location,
        buffer,
        Some(alpha),
        None,
        Some(size),
        Kind::Unspecified,
    ) {
        Ok(element) => elements.push(element.into()),
        Err(error) => tracing::warn!(%error, "failed to import a decoration glyph buffer"),
    }
}

pub fn rgba(color: [u8; 4]) -> Color32F {
    Color32F::new(
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    )
}

pub fn to_rect(rect: Rectangle<i32, Logical>) -> Rect {
    Rect {
        x: rect.loc.x,
        y: rect.loc.y,
        width: rect.size.w,
        height: rect.size.h,
    }
}

pub fn from_rect(rect: Rect) -> Rectangle<i32, Logical> {
    Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into())
}

fn for_each_surface_root(state: &BlairState, mut f: impl FnMut(&WlSurface)) {
    for toplevel in state.xdg_shell_state.toplevel_surfaces() {
        let surface = toplevel.wl_surface();
        f(surface);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            f(popup.wl_surface());
        }
    }
    for layer in &state.layer_surfaces {
        let surface = layer.wl_surface();
        f(surface);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            f(popup.wl_surface());
        }
    }
    if let CursorImageStatus::Surface(surface) = &state.pointer_cursor() {
        f(surface);
    }
    if let Some(icon) = &state.dnd_icon {
        f(icon);
    }
}

pub fn update_primary_scanout_output(
    state: &BlairState,
    output: &Output,
    render_states: &RenderElementStates,
) {
    for_each_surface_root(state, |root| {
        with_surfaces_surface_tree(root, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_states,
                default_primary_scanout_output_compare,
            );
        });
    });
}

pub fn send_frame_callbacks(state: &BlairState, output: &Output, time: Duration) {
    profiling::scope!("send_frame_callbacks");
    for_each_surface_root(state, |root| {
        send_frames_surface_tree(
            root,
            output,
            time,
            Some(HIDDEN_FRAME_THROTTLE),
            surface_primary_scanout_output,
        );
    });
}

pub fn take_presentation_feedback(
    state: &BlairState,
    output: &Output,
    render_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);
    for_each_surface_root(state, |root| {
        take_presentation_feedback_surface_tree(
            root,
            &mut feedback,
            surface_primary_scanout_output,
            |surface, _| surface_presentation_feedback_flags_from_states(surface, render_states),
        );
    });
    feedback
}
