use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{memory::MemoryRenderBuffer, Element, Id, Kind, RenderElement},
            gles::{
                GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture,
                Uniform, UniformName, UniformType,
            },
            utils::{CommitCounter, DamageBag, DamageSet, DamageSnapshot},
            Color32F,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::decorations::RgbaBitmap;

const FRAME_SHADER: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform float radius;
uniform float border_width;
uniform float titlebar_height;
uniform vec4 border_color;
uniform vec4 titlebar_color;

float rounded_rect(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + vec2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

void main() {
    vec2 pos = v_coords * size;
    vec2 c = pos - size * 0.5;
    float outer = clamp(0.5 - rounded_rect(c, size * 0.5, radius), 0.0, 1.0);
    vec2 inner_half = max(size * 0.5 - vec2(border_width), vec2(0.0));
    float inner_radius = max(radius - border_width, 0.0);
    float inside = clamp(0.5 - rounded_rect(c, inner_half, inner_radius), 0.0, 1.0);
    float ring = outer * (1.0 - inside);
    float titlebar = inside * step(pos.y, border_width + titlebar_height);
    gl_FragColor = (border_color * ring + titlebar_color * titlebar) * alpha;
}
"#;

const TITLEBAR_BLEND_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
uniform mat3 titlebar_from_ndc;
uniform vec2 viewport;
uniform vec2 titlebar_size;
uniform float radius;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float top_corner_coverage(vec2 pos) {
    float r = min(max(radius, 0.0), titlebar_size.x * 0.5);
    if (r == 0.0 || pos.y >= r || (pos.x >= r && pos.x <= titlebar_size.x - r))
        return 1.0;
    vec2 center = pos.x < r ? vec2(r, r) : vec2(titlebar_size.x - r, r);
    return clamp(0.5 - (length(pos - center) - r), 0.0, 1.0);
}

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    vec2 ndc = gl_FragCoord.xy / viewport * 2.0 - 1.0;
    vec2 pos = (titlebar_from_ndc * vec3(ndc, 1.0)).xy;
    color *= alpha * top_corner_coverage(pos);
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

pub fn compile(renderer: &mut GlesRenderer) -> Result<GlesPixelProgram, GlesError> {
    renderer.compile_custom_pixel_shader(
        FRAME_SHADER,
        &[
            UniformName::new("radius", UniformType::_1f),
            UniformName::new("border_width", UniformType::_1f),
            UniformName::new("titlebar_height", UniformType::_1f),
            UniformName::new("border_color", UniformType::_4f),
            UniformName::new("titlebar_color", UniformType::_4f),
        ],
    )
}

pub fn compile_titlebar_blend(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    renderer.compile_custom_texture_shader(
        TITLEBAR_BLEND_SHADER,
        &[
            UniformName::new("titlebar_from_ndc", UniformType::Matrix3x3),
            UniformName::new("viewport", UniformType::_2f),
            UniformName::new("titlebar_size", UniformType::_2f),
            UniformName::new("radius", UniformType::_1f),
        ],
    )
}

/// Logical-unit frame parameters; colors are straight (non-premultiplied).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameParams {
    pub radius: f32,
    pub border_width: f32,
    pub titlebar_height: f32,
    pub border_color: Color32F,
    pub titlebar_color: Color32F,
}

#[derive(Clone)]
pub struct TitlebarPixelSource {
    pub texture: GlesTexture,
    pub row: Rectangle<f64, Buffer>,
    pub transform: Transform,
    pub program: GlesTexProgram,
}

/// A 1px-tall offscreen copy of a row near the top of the window's content,
/// kept across frames so it is only re-rendered when the client commits
/// something that touches that row.
pub struct TitlebarStrip {
    pub texture: GlesTexture,
    pub tracker: OutputDamageTracker,
    pub size: Size<i32, Physical>,
    pub scale: f64,
    /// Whether `texture` holds a previous render, so the tracker can repaint
    /// only damage instead of the whole strip.
    pub rendered: bool,
    /// Average premultiplied color of the strip, read back when it changes.
    pub average: Option<[u8; 4]>,
}

impl std::fmt::Debug for TitlebarStrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TitlebarStrip")
            .field("size", &self.size)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

/// Per-window decoration state kept across frames so render element ids
/// and commit counters stay stable and damage tracking only repaints what
/// changed.
#[derive(Debug)]
pub struct WindowDecoration {
    id: Id,
    /// Damage local to `area`; a reset (frame moved, resized, or restyled)
    /// makes the whole frame dirty, while strip updates only dirty the
    /// titlebar.
    damage: DamageBag<i32, Logical>,
    area: Rectangle<i32, Logical>,
    params: Option<FrameParams>,
    pub strip: Option<TitlebarStrip>,
    buttons: [BufferCache; 3],
    title: BufferCache,
    icon: BufferCache,
}

#[derive(Debug, Default)]
struct BufferCache {
    key: Option<String>,
    buffer: Option<MemoryRenderBuffer>,
    size: Size<i32, Logical>,
}

impl Default for WindowDecoration {
    fn default() -> Self {
        Self {
            id: Id::new(),
            damage: DamageBag::default(),
            area: Rectangle::default(),
            params: None,
            strip: None,
            buttons: Default::default(),
            title: BufferCache::default(),
            icon: BufferCache::default(),
        }
    }
}

impl WindowDecoration {
    pub fn update(&mut self, area: Rectangle<i32, Logical>, params: FrameParams) {
        if self.area != area || self.params != Some(params) {
            self.area = area;
            self.params = Some(params);
            self.damage.reset();
        }
    }

    /// Marks only the titlebar as dirty, after the sampled strip changed.
    pub fn damage_titlebar(&mut self) {
        let Some(params) = self.params else {
            return;
        };
        let border = params.border_width.round() as i32;
        let height = params.titlebar_height.ceil() as i32;
        let width = self.area.size.w - border * 2;
        if width > 0 && height > 0 {
            self.damage.add([Rectangle::new(
                (border, border).into(),
                (width, height).into(),
            )]);
        }
    }

    /// Rasterizes `text` into the title glyph cache when the cache key
    /// (text, color, box, alignment, and scale) changed since the last call.
    /// `size`, `max_width` and `height` are logical; the bitmap is drawn at
    /// `buffer_scale` so it stays crisp on HiDPI outputs.
    #[allow(clippy::too_many_arguments)]
    pub fn update_title(
        &mut self,
        font: &fontdue::Font,
        text: &str,
        size: f32,
        color: [u8; 4],
        max_width: i32,
        height: i32,
        centered: bool,
        buffer_scale: i32,
    ) {
        let key = format!("{text}|{size}|{color:?}|{max_width}|{height}|{centered}|{buffer_scale}");
        if self.title.key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.title.key = Some(key);
        let glyphs = crate::decorations::rasterize_title(
            font,
            text,
            size * buffer_scale as f32,
            color,
            max_width * buffer_scale,
            height * buffer_scale,
            centered,
        );
        self.title.buffer = glyphs.as_ref().map(|glyphs| {
            buffer_from_pixels(
                &premultiply(&glyphs.pixels),
                glyphs.width,
                glyphs.height,
                buffer_scale,
            )
        });
        self.title.size = glyphs
            .map(|glyphs| (glyphs.width / buffer_scale, glyphs.height / buffer_scale).into())
            .unwrap_or_default();
    }

    /// Uploads a window's real app icon into the icon buffer, keyed by its
    /// identity so it is only re-uploaded when the icon itself changes.
    /// Clears the buffer when `icon` is `None`, so a window with no
    /// resolvable icon renders nothing rather than a placeholder. `icon` is
    /// expected at `buffer_scale` times the logical `size`.
    pub fn update_icon(
        &mut self,
        key: &str,
        icon: Option<&RgbaBitmap>,
        size: i32,
        buffer_scale: i32,
    ) {
        if self.icon.key.as_deref() == Some(key) {
            return;
        }
        self.icon.key = Some(key.to_owned());
        self.icon.buffer = icon.map(|icon| {
            buffer_from_pixels(
                &premultiply(&icon.pixels),
                icon.width,
                icon.height,
                buffer_scale,
            )
        });
        self.icon.size = icon.map(|_| (size, size).into()).unwrap_or_default();
    }

    /// Rasterizes a titlebar button into slot `index` when its look, size,
    /// or scale changed since the last call.
    pub fn update_button(&mut self, index: usize, look: ButtonLook, size: i32, buffer_scale: i32) {
        let key = format!("{look:?}|{size}|{buffer_scale}");
        let cache = &mut self.buttons[index];
        if cache.key.as_deref() == Some(key.as_str()) {
            return;
        }
        cache.key = Some(key);
        if size <= 0 {
            cache.buffer = None;
            cache.size = Default::default();
            return;
        }
        let pixels = size * buffer_scale;
        cache.buffer = Some(buffer_from_pixels(
            &rasterize_button(look, pixels),
            pixels,
            pixels,
            buffer_scale,
        ));
        cache.size = (size, size).into();
    }

    pub fn button_buffer(&self, index: usize) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>)> {
        let cache = &self.buttons[index];
        cache.buffer.as_ref().map(|buffer| (buffer, cache.size))
    }

    pub fn title_buffer(&self) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>)> {
        self.title
            .buffer
            .as_ref()
            .map(|buffer| (buffer, self.title.size))
    }

    pub fn icon_buffer(&self) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>)> {
        self.icon
            .buffer
            .as_ref()
            .map(|buffer| (buffer, self.icon.size))
    }

    pub fn element(
        &self,
        program: &GlesPixelProgram,
        output_origin: smithay::utils::Point<i32, Logical>,
        scale: f64,
        alpha: f32,
        titlebar_source: Option<&TitlebarPixelSource>,
    ) -> Option<FrameElement> {
        let params = self.params?;
        Some(FrameElement {
            id: self.id.clone(),
            damage: self.damage.snapshot(),
            geometry: Rectangle::new(self.area.loc - output_origin, self.area.size)
                .to_physical_precise_round(scale),
            scale,
            params,
            alpha,
            program: program.clone(),
            titlebar_source: titlebar_source.cloned(),
        })
    }
}

pub struct FrameElement {
    id: Id,
    damage: DamageSnapshot<i32, Logical>,
    geometry: Rectangle<i32, Physical>,
    scale: f64,
    params: FrameParams,
    alpha: f32,
    program: GlesPixelProgram,
    titlebar_source: Option<TitlebarPixelSource>,
}

impl Element for FrameElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self.damage.damage_since(commit) {
            Some(damage) => damage
                .into_iter()
                .map(|rect| rect.to_f64().to_physical(self.scale).to_i32_up())
                .collect(),
            None => DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(
            self.geometry
                .size
                .to_f64()
                .to_logical(1.0)
                .to_buffer(1.0, Transform::Normal),
        )
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for FrameElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let scale = self.scale as f32;
        let params = &self.params;
        let size: Size<i32, Buffer> = (dst.size.w, dst.size.h).into();
        frame.render_pixel_shader_to(
            &self.program,
            src,
            dst,
            size,
            Some(damage),
            self.alpha,
            &[
                Uniform::new("radius", params.radius * scale),
                Uniform::new("border_width", params.border_width * scale),
                Uniform::new("titlebar_height", params.titlebar_height * scale),
                Uniform::new("border_color", premultiplied(params.border_color)),
                Uniform::new("titlebar_color", premultiplied(params.titlebar_color)),
            ],
        )?;

        let Some(source) = &self.titlebar_source else {
            return Ok(());
        };
        let border = (params.border_width * scale).round() as i32;
        let height = (params.titlebar_height * scale).round() as i32;
        let width = (dst.size.w - border * 2).max(0);
        if width == 0 || height == 0 {
            return Ok(());
        }
        let titlebar = Rectangle::new(
            (dst.loc.x + border, dst.loc.y + border).into(),
            (width, height).into(),
        );
        // `damage` is relative to `dst`; the texture draw wants it relative
        // to the titlebar rect.
        let titlebar_local = Rectangle::from_size(titlebar.size);
        let titlebar_damage: Vec<_> = damage
            .iter()
            .filter_map(|rect| {
                let rect = Rectangle::new(
                    rect.loc - Point::<i32, Physical>::from((border, border)),
                    rect.size,
                );
                rect.intersection(titlebar_local)
            })
            .collect();
        if titlebar_damage.is_empty() {
            return Ok(());
        }
        frame.render_texture_from_to(
            &source.texture,
            source.row,
            titlebar,
            &titlebar_damage,
            &[],
            source.transform,
            self.alpha,
            Some(&source.program),
            &[
                Uniform::new(
                    "titlebar_from_ndc",
                    smithay::backend::renderer::gles::UniformValue::Matrix3x3 {
                        matrices: vec![super::clip::local_from_ndc(titlebar, frame.projection())],
                        transpose: false,
                    },
                ),
                Uniform::new(
                    "viewport",
                    (
                        2.0 / frame.projection()[0].abs(),
                        2.0 / frame.projection()[4].abs(),
                    ),
                ),
                Uniform::new("titlebar_size", (width as f32, height as f32)),
                Uniform::new(
                    "radius",
                    (params.radius - params.border_width).max(0.0) * scale,
                ),
            ],
        )
    }
}

fn premultiplied(color: Color32F) -> (f32, f32, f32, f32) {
    let a = color.a();
    (color.r() * a, color.g() * a, color.b() * a, a)
}

/// Smithay treats uploaded RGBA as premultiplied, so every bitmap passed
/// here must already be premultiplied (see [`premultiply`]); uploading
/// straight alpha brightens every antialiased edge and makes text look bold
/// and jagged.
fn buffer_from_pixels(pixels: &[u8], width: i32, height: i32, scale: i32) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        pixels,
        Fourcc::Abgr8888,
        (width, height),
        scale,
        Transform::Normal,
        None,
    )
}

fn premultiply(pixels: &[u8]) -> Vec<u8> {
    pixels
        .chunks_exact(4)
        .flat_map(|px| {
            let a = u16::from(px[3]);
            let channel = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
            [channel(px[0]), channel(px[1]), channel(px[2]), px[3]]
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonGlyph {
    Close,
    Maximize,
    Minimize,
}

/// How a titlebar button is drawn: a soft disc with a thin glyph on top.
/// Colors are straight (non-premultiplied) RGBA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonLook {
    pub glyph: ButtonGlyph,
    pub disc: [u8; 4],
    pub glyph_color: [u8; 4],
}

/// Renders `look` into a premultiplied `size`x`size` bitmap.
fn rasterize_button(look: ButtonLook, size: i32) -> Vec<u8> {
    let size = size.max(1);
    let mut pixels = vec![0u8; size as usize * size as usize * 4];
    let s = size as f32;
    let radius = s / 2.0;
    // Glyphs span a bit over a third of the disc, with a hairline stroke
    // that still survives at 1x.
    let extent = s * 0.18;
    let stroke = (s * 0.075).max(1.0);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5 - radius;
            let py = y as f32 + 0.5 - radius;
            let disc = (0.5 - ((px * px + py * py).sqrt() - (radius - 0.5))).clamp(0.0, 1.0);
            if disc <= 0.0 {
                continue;
            }
            let distance = match look.glyph {
                ButtonGlyph::Close => segment_distance(px, py, -extent, -extent, extent, extent)
                    .min(segment_distance(px, py, -extent, extent, extent, -extent)),
                ButtonGlyph::Minimize => segment_distance(px, py, -extent, 0.0, extent, 0.0),
                ButtonGlyph::Maximize => {
                    let side = extent * 0.9;
                    (px.abs().max(py.abs()) - side).abs()
                }
            };
            let glyph = (stroke / 2.0 + 0.5 - distance).clamp(0.0, 1.0) * disc;

            let offset = (y as usize * size as usize + x as usize) * 4;
            let base_alpha = f32::from(look.disc[3]) / 255.0 * disc;
            let glyph_alpha = f32::from(look.glyph_color[3]) / 255.0 * glyph;
            for channel in 0..3 {
                let base = f32::from(look.disc[channel]) * base_alpha;
                let top = f32::from(look.glyph_color[channel]) * glyph_alpha;
                pixels[offset + channel] = (top + base * (1.0 - glyph_alpha)).round() as u8;
            }
            pixels[offset + 3] =
                ((glyph_alpha + base_alpha * (1.0 - glyph_alpha)) * 255.0).round() as u8;
        }
    }
    pixels
}

fn segment_distance(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t =
        (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy).max(f32::EPSILON)).clamp(0.0, 1.0);
    let (cx, cy) = (ax + dx * t - px, ay + dy * t - py);
    (cx * cx + cy * cy).sqrt()
}

/// WCAG relative luminance of an sRGB color, ignoring alpha.
pub fn luminance(color: [u8; 4]) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

/// A near-black or near-white foreground, whichever contrasts more with
/// `background`.
pub fn contrasting(background: [u8; 4]) -> [u8; 4] {
    let l = luminance(background);
    // Contrast against white is 1.05 / (l + 0.05), against black
    // (l + 0.05) / 0.05; they cross at l ≈ 0.179.
    if (l + 0.05) / 0.05 > 1.05 / (l + 0.05) {
        [0x1d, 0x1d, 0x1f, 0xff]
    } else {
        [0xf5, 0xf5, 0xf7, 0xff]
    }
}

/// Blends `a` towards `b` by `t` (0 keeps `a`, 1 fully becomes `b`).
pub fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [
        lerp(a[0], b[0]),
        lerp(a[1], b[1]),
        lerp(a[2], b[2]),
        lerp(a[3], b[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_is_a_disc_with_a_glyph_and_empty_corners() {
        let size = 20;
        let look = ButtonLook {
            glyph: ButtonGlyph::Close,
            disc: [0, 0, 0, 0],
            glyph_color: [255, 255, 255, 255],
        };
        let pixels = rasterize_button(look, size);
        let center = (10 * size as usize + 10) * 4;
        // The cross passes through the center at full coverage.
        assert!(pixels[center + 3] > 200);
        assert_eq!(pixels[3], 0);
        // Premultiplied: no channel may exceed alpha.
        assert!(pixels
            .chunks_exact(4)
            .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));
    }

    #[test]
    fn premultiply_scales_color_by_alpha() {
        assert_eq!(premultiply(&[255, 128, 0, 128]), vec![128, 64, 0, 128]);
    }

    #[test]
    fn contrast_picks_dark_text_on_light_backgrounds() {
        assert_eq!(contrasting([250, 250, 250, 255])[0], 0x1d);
        assert_eq!(contrasting([20, 20, 30, 255])[0], 0xf5);
    }

    #[test]
    fn mix_interpolates_each_channel() {
        assert_eq!(
            mix([0, 0, 0, 255], [255, 255, 255, 255], 0.0),
            [0, 0, 0, 255]
        );
        assert_eq!(
            mix([0, 0, 0, 255], [255, 255, 255, 255], 1.0),
            [255, 255, 255, 255]
        );
        assert_eq!(
            mix([0, 0, 0, 0], [100, 200, 50, 255], 0.5),
            [50, 100, 25, 128]
        );
    }
}
