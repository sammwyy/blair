//! Renders the background blur `blair_blur_v1` requests (see [`crate::blur`]):
//! captures whatever is behind a blurred window into an offscreen texture,
//! then draws that texture back with a custom blur fragment shader, right
//! behind the window's own (typically translucent) content.

use smithay::{
    backend::renderer::{
        element::{
            texture::TextureRenderElement, Element, Id, Kind, RenderElement, UnderlyingStorage,
        },
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
            UniformType, UniformValue,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Renderer, Texture,
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform},
};

use super::{clip::local_from_ndc, FrameContext, OutputRenderElement, RoundedClip, Shaders};
use crate::state::BlairState;

const BLUR_SHADER: &str = r#"#version 100

precision highp float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_coords;

uniform vec2 texel;
uniform float spread;
uniform vec2 direction;
uniform mat3 clip_from_ndc;
uniform vec2 viewport;
uniform vec2 clip_size;
uniform vec4 clip_radii;

float coverage(vec2 p) {
    vec2 half_size = clip_size * 0.5;
    vec2 c = p - half_size;
    float r = c.x < 0.0
        ? (c.y < 0.0 ? clip_radii.x : clip_radii.w)
        : (c.y < 0.0 ? clip_radii.y : clip_radii.z);
    vec2 q = abs(c) - half_size + vec2(r);
    float d = min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
    return clamp(0.5 - d, 0.0, 1.0);
}

void main() {
    vec2 low = texel * 0.5;
    vec2 high = vec2(1.0) - low;
    vec4 sum = texture2D(tex, clamp(v_coords, low, high));
    float total = 1.0;
    float sigma = max(3.0 * spread, 0.001);
    float ratio = exp(-0.5 / (sigma * sigma));
    float step_ratio = ratio * ratio;
    float weight = 1.0;
    float support = ceil(4.0 * spread);
    for (int i = 1; i <= 256; i += 2) {
        if (float(i) > support) break;
        weight *= ratio;
        ratio *= step_ratio;
        float next_weight = float(i + 1) <= support ? weight * ratio : 0.0;
        float pair_weight = weight + next_weight;
        float offset = float(i) + next_weight / pair_weight;
        vec2 delta = direction * texel * offset;
        sum += (texture2D(tex, clamp(v_coords + delta, low, high))
            + texture2D(tex, clamp(v_coords - delta, low, high))) * pair_weight;
        total += 2.0 * pair_weight;
        weight *= ratio;
        ratio *= step_ratio;
    }
    vec2 ndc = gl_FragCoord.xy / viewport * 2.0 - 1.0;
    gl_FragColor = (sum / total) * alpha
        * coverage((clip_from_ndc * vec3(ndc, 1.0)).xy);
}
"#;

pub fn compile(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    renderer.compile_custom_texture_shader(
        BLUR_SHADER,
        &[
            UniformName::new("texel", UniformType::_2f),
            UniformName::new("spread", UniformType::_1f),
            UniformName::new("direction", UniformType::_2f),
            UniformName::new("clip_from_ndc", UniformType::Matrix3x3),
            UniformName::new("viewport", UniformType::_2f),
            UniformName::new("clip_size", UniformType::_2f),
            UniformName::new("clip_radii", UniformType::_4f),
        ],
    )
}

/// Captures the already assembled scene below this surface, including its blur.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_backdrop(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    backdrop: &mut [OutputRenderElement],
    frame_rect: Rectangle<i32, Logical>,
    coverage: &[Rectangle<i32, Logical>],
    clip: Option<RoundedClip>,
    elements: &mut Vec<OutputRenderElement>,
) {
    let Some(shaders) = shaders else { return };
    if !state.config.blur.enabled {
        return;
    }
    let radius = state.config.blur.radius as f32;
    if radius <= 0.0 {
        return;
    }
    let Some(frame_rect_physical) = frame_rect
        .to_physical_precise_round(ctx.scale)
        .intersection(Rectangle::from_size(ctx.viewport))
    else {
        return;
    };
    if frame_rect_physical.is_empty() {
        return;
    }

    // Keep every shader tap and its bilinear filtering footprint inside the capture.
    let capture = backdrop_region(frame_rect_physical, ctx.viewport, radius);
    let target = match super::capture::render_region(renderer, backdrop, capture, ctx.scale) {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(%error, "failed to capture blur backdrop");
            return;
        }
    };

    let columns = Rectangle::new(
        (frame_rect_physical.loc.x - capture.loc.x, 0).into(),
        (frame_rect_physical.size.w, capture.size.h).into(),
    );
    let target = match horizontal_blur(renderer, target, &shaders.blur, radius, columns) {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(%error, "failed to filter blur backdrop");
            return;
        }
    };
    let texel = (1.0 / columns.size.w as f32, 1.0 / columns.size.h as f32);
    let src = Rectangle::<f64, Logical>::new(
        Point::from((0.0, (frame_rect_physical.loc.y - capture.loc.y) as f64)),
        (
            frame_rect_physical.size.w as f64,
            frame_rect_physical.size.h as f64,
        )
            .into(),
    );
    let inner = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        frame_rect_physical.loc.to_f64(),
        target,
        1,
        Transform::Normal,
        Some(1.0),
        Some(src),
        Some(frame_rect.size),
        None,
        Kind::Unspecified,
    );
    let coverage = coverage
        .iter()
        .filter_map(|rect| {
            rect.to_physical_precise_round(ctx.scale)
                .intersection(frame_rect_physical)
                .map(|rect| Rectangle::new(rect.loc - frame_rect_physical.loc, rect.size))
        })
        .collect();
    let clip = clip.unwrap_or(RoundedClip {
        rect: Rectangle::from_size(ctx.viewport),
        radii: [0.0; 4],
        viewport: ctx.viewport,
    });
    elements.push(
        BlurredBackdropElement::new(inner, shaders.blur.clone(), texel, radius, coverage, clip)
            .into(),
    );
}

fn horizontal_blur(
    renderer: &mut GlesRenderer,
    texture: GlesTexture,
    program: &GlesTexProgram,
    radius: f32,
    columns: Rectangle<i32, Physical>,
) -> anyhow::Result<GlesTexture> {
    let size = texture
        .size()
        .to_logical(1, Transform::Normal)
        .to_physical(1);
    let inner = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture,
        1,
        Transform::Normal,
        None,
        None,
        None,
        None,
        Kind::Unspecified,
    );
    let mut element = BlurredBackdropElement::new(
        inner,
        program.clone(),
        (1.0 / size.w as f32, 1.0 / size.h as f32),
        radius,
        vec![Rectangle::from_size(size)],
        RoundedClip {
            rect: Rectangle::from_size(size),
            radii: [0.0; 4],
            viewport: size,
        },
    );
    element.direction = (1.0, 0.0);
    super::capture::render_region(renderer, &mut [element.into()], columns, 1.0)
}

fn backdrop_region(
    frame: Rectangle<i32, Physical>,
    viewport: smithay::utils::Size<i32, Physical>,
    radius: f32,
) -> Rectangle<i32, Physical> {
    let padding = (radius * 4.0).ceil() as i32 + 1;
    let left = (frame.loc.x - padding).max(0);
    let top = (frame.loc.y - padding).max(0);
    let right = (frame.loc.x + frame.size.w + padding).min(viewport.w);
    let bottom = (frame.loc.y + frame.size.h + padding).min(viewport.h);
    Rectangle::new((left, top).into(), (right - left, bottom - top).into())
}

pub struct BlurredBackdropElement {
    inner: TextureRenderElement<GlesTexture>,
    program: GlesTexProgram,
    texel: (f32, f32),
    spread: f32,
    direction: (f32, f32),
    coverage: Vec<Rectangle<i32, Physical>>,
    clip: RoundedClip,
    offscreen_region: Option<Rectangle<i32, Physical>>,
}

impl BlurredBackdropElement {
    pub(super) fn set_offscreen_region(&mut self, region: Option<Rectangle<i32, Physical>>) {
        self.offscreen_region = region;
    }

    fn new(
        inner: TextureRenderElement<GlesTexture>,
        program: GlesTexProgram,
        texel: (f32, f32),
        spread: f32,
        coverage: Vec<Rectangle<i32, Physical>>,
        clip: RoundedClip,
    ) -> Self {
        Self {
            inner,
            program,
            texel,
            spread,
            direction: (0.0, 1.0),
            coverage,
            clip,
            offscreen_region: None,
        }
    }
}

impl Element for BlurredBackdropElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for BlurredBackdropElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let damage: Vec<_> = damage
            .iter()
            .flat_map(|damage| {
                self.coverage
                    .iter()
                    .filter_map(move |region| damage.intersection(*region))
            })
            .collect();
        if damage.is_empty() {
            return Ok(());
        }
        let mut clip = self.clip;
        if let Some(region) = self.offscreen_region {
            clip.rect.loc -= region.loc;
            clip.viewport = region.size;
        }
        let [tl, tr, br, bl] = clip.radii;
        let uniforms = vec![
            Uniform::new("texel", self.texel),
            Uniform::new("spread", self.spread),
            Uniform::new("direction", self.direction),
            Uniform::new(
                "clip_from_ndc",
                UniformValue::Matrix3x3 {
                    matrices: vec![local_from_ndc(clip.rect, frame.projection())],
                    transpose: false,
                },
            ),
            Uniform::new("viewport", (clip.viewport.w as f32, clip.viewport.h as f32)),
            Uniform::new(
                "clip_size",
                (self.clip.rect.size.w as f32, self.clip.rect.size.h as f32),
            ),
            Uniform::new("clip_radii", (tl, tr, br, bl)),
        ];
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let result = RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            &damage,
            opaque_regions,
        );
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod scene_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::{
        allocator::Fourcc,
        renderer::{damage::OutputDamageTracker, Bind, Color32F, ExportMem, ImportMem, Offscreen},
    };

    #[test]
    fn backdrop_capture_includes_filter_padding_and_clamps_to_output() {
        let viewport = (1920, 1080).into();
        let frame = Rectangle::new((500, 400).into(), (200, 32).into());
        let capture = backdrop_region(frame, viewport, 8.0);
        assert_eq!(capture, Rectangle::new((467, 367).into(), (266, 98).into()));
        assert!(capture.size.w * capture.size.h * 4 < 105_000);
        assert_eq!(
            backdrop_region(Rectangle::from_size(viewport), viewport, 64.0),
            Rectangle::from_size(viewport)
        );
        assert_eq!(
            backdrop_region(
                Rectangle::new((1900, 1070).into(), (20, 10).into()),
                viewport,
                8.0
            ),
            Rectangle::new((1867, 1037).into(), (53, 43).into())
        );
    }

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn cropped_backdrops_match_full_output_with_padding_and_relocated_clips() {
        use smithay::backend::renderer::Texture;

        let egl = crate::test_support::EglTest::new();
        let mut renderer = egl.renderer();
        let program = compile(&mut renderer).unwrap();
        let viewport = (240, 160).into();
        let pixels: Vec<u8> = (0..160)
            .flat_map(|y| {
                (0..240).flat_map(move |x| {
                    let value = if (x / 7 + y / 11) % 2 == 0 { 16 } else { 240 };
                    [value, 255 - value, value, 255]
                })
            })
            .collect();
        let texture = renderer
            .import_memory(&pixels, Fourcc::Abgr8888, (240, 160).into(), false)
            .unwrap();

        for scale in [1.0, 1.25, 2.0] {
            for radius in [0.75, 2.0, 8.0] {
                for location in [(80, 60), (0, 0), (200, 130)] {
                    let frame = Rectangle::new(location.into(), (40, 30).into());
                    let crop = backdrop_region(frame, viewport, radius);
                    let mut results = Vec::new();
                    for capture in [Rectangle::from_size(viewport), crop] {
                        let backdrop = TextureRenderElement::from_static_texture(
                            Id::new(),
                            renderer.context_id(),
                            (0.0, 0.0),
                            texture.clone(),
                            1,
                            Transform::Normal,
                            None,
                            None,
                            Some(viewport.to_f64().to_logical(scale).to_i32_round()),
                            None,
                            Kind::Unspecified,
                        );
                        let target = super::super::capture::render_region(
                            &mut renderer,
                            &mut [backdrop.into()],
                            capture,
                            scale,
                        )
                        .unwrap();
                        assert_eq!(target.size().w, capture.size.w);
                        assert_eq!(target.size().h, capture.size.h);
                        let columns = Rectangle::new(
                            (frame.loc.x - capture.loc.x, 0).into(),
                            (frame.size.w, capture.size.h).into(),
                        );
                        let target =
                            horizontal_blur(&mut renderer, target, &program, radius, columns)
                                .unwrap();
                        let inner = TextureRenderElement::from_static_texture(
                            Id::new(),
                            renderer.context_id(),
                            frame.loc.to_f64(),
                            target,
                            1,
                            Transform::Normal,
                            None,
                            Some(Rectangle::new(
                                Point::from((0.0, (frame.loc.y - capture.loc.y) as f64)),
                                frame.size.to_f64().to_logical(1.0),
                            )),
                            Some(frame.size.to_f64().to_logical(scale).to_i32_round()),
                            None,
                            Kind::Unspecified,
                        );
                        let element = BlurredBackdropElement::new(
                            inner,
                            program.clone(),
                            (1.0 / columns.size.w as f32, 1.0 / columns.size.h as f32),
                            radius,
                            vec![Rectangle::from_size(frame.size)],
                            RoundedClip {
                                rect: frame,
                                radii: [9.0; 4],
                                viewport,
                            },
                        );
                        // Cropping the final draw also exercises clips in nested offscreen passes.
                        let mut rendered = super::super::capture::render_region(
                            &mut renderer,
                            &mut [element.into()],
                            frame,
                            scale,
                        )
                        .unwrap();
                        let framebuffer = renderer.bind(&mut rendered).unwrap();
                        let mapping = renderer
                            .copy_framebuffer(
                                &framebuffer,
                                Rectangle::from_size(
                                    frame.size.to_logical(1).to_buffer(1, Transform::Normal),
                                ),
                                Fourcc::Abgr8888,
                            )
                            .unwrap();
                        results.push(renderer.map_texture(&mapping).unwrap().to_vec());
                    }
                    let difference = results[0]
                        .iter()
                        .zip(&results[1])
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(difference <= 2,
                        "crop differs by {difference}: scale={scale}, radius={radius}, frame={frame:?}");
                }
            }
        }
    }

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn separable_gaussian_matches_dense_reference_at_small_and_large_radii() {
        let egl = crate::test_support::EglTest::new();
        let mut renderer = egl.renderer();
        let program = compile(&mut renderer).unwrap();
        let size = (64, 48).into();
        let pixels: Vec<u8> = (0..48)
            .flat_map(|y| {
                (0..64).flat_map(move |x| {
                    let value = if (x / 8 + y / 8) % 2 == 0 { 0 } else { 255 };
                    [value, 255 - value, value, 255]
                })
            })
            .collect();
        for radius in [1.0_f32, 4.0, 16.0, 64.0] {
            let texture = renderer
                .import_memory(&pixels, Fourcc::Abgr8888, (64, 48).into(), false)
                .unwrap();
            let texture = horizontal_blur(
                &mut renderer,
                texture,
                &program,
                radius,
                Rectangle::from_size(size),
            )
            .unwrap();
            let inner = TextureRenderElement::from_static_texture(
                Id::new(),
                renderer.context_id(),
                (0.0, 0.0),
                texture,
                1,
                Transform::Normal,
                None,
                None,
                None,
                None,
                Kind::Unspecified,
            );
            let element = BlurredBackdropElement::new(
                inner,
                program.clone(),
                (1.0 / 64.0, 1.0 / 48.0),
                radius,
                vec![Rectangle::from_size(size)],
                RoundedClip {
                    rect: Rectangle::from_size(size),
                    radii: [0.0; 4],
                    viewport: size,
                },
            );
            let mut rendered = super::super::capture::render_region(
                &mut renderer,
                &mut [element.into()],
                Rectangle::from_size(size),
                1.0,
            )
            .unwrap();
            let framebuffer = renderer.bind(&mut rendered).unwrap();
            let mapping = renderer
                .copy_framebuffer(
                    &framebuffer,
                    Rectangle::from_size((64, 48).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let result = renderer.map_texture(&mapping).unwrap();
            let support = (4.0 * radius).ceil() as i32;
            let weights: Vec<_> = (-support..=support)
                .map(|i| (-(i as f64).powi(2) / (18.0 * (radius as f64).powi(2))).exp())
                .collect();
            let total: f64 = weights.iter().sum();
            let mut reference = pixels.clone();
            for (dx, dy) in [(1, 0), (0, 1)] {
                let mut filtered = reference.clone();
                for y in 0..48 {
                    for x in 0..64 {
                        for channel in 0..3 {
                            let sum: f64 = (-support..=support)
                                .zip(&weights)
                                .map(|(offset, weight)| {
                                    let sx = (x + dx * offset).clamp(0, 63);
                                    let sy = (y + dy * offset).clamp(0, 47);
                                    reference[((sy * 64 + sx) * 4 + channel) as usize] as f64
                                        * weight
                                })
                                .sum();
                            filtered[((y * 64 + x) * 4 + channel) as usize] =
                                (sum / total).round() as u8;
                        }
                    }
                }
                reference = filtered;
            }
            let difference = result
                .iter()
                .zip(&reference)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(difference <= 2, "radius={radius}, difference={difference}");
            if radius >= 16.0 {
                assert!((120..=135).contains(&result[(24 * 64 + 32) * 4]));
            }
        }
    }

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn popup_blur_softens_the_backdrop_and_preserves_rounded_corners() {
        let egl = crate::test_support::EglTest::new();
        let mut renderer = egl.renderer();
        let program = compile(&mut renderer).unwrap();
        let viewport: smithay::utils::Size<i32, Physical> = (64, 64).into();
        let mut pixels = Vec::new();
        for _y in 0..64 {
            for x in 0..64 {
                let value = if x < 32 { 0 } else { 255 };
                pixels.extend([value, value, value, 255]);
            }
        }
        let texture = renderer
            .import_memory(&pixels, Fourcc::Abgr8888, (64, 64).into(), false)
            .unwrap();
        let texture = horizontal_blur(
            &mut renderer,
            texture,
            &program,
            2.0,
            Rectangle::from_size(viewport),
        )
        .unwrap();
        let inner = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            (0.0, 0.0),
            texture,
            1,
            Transform::Normal,
            Some(1.0),
            None,
            None,
            None,
            Kind::Unspecified,
        );
        let clip = RoundedClip {
            rect: Rectangle::new((8, 8).into(), (48, 48).into()),
            radii: [12.0; 4],
            viewport,
        };
        let element = BlurredBackdropElement::new(
            inner,
            program,
            (1.0 / 64.0, 1.0 / 64.0),
            2.0,
            vec![Rectangle::from_size(viewport)],
            clip,
        );
        let mut target = Offscreen::<GlesTexture>::create_buffer(
            &mut renderer,
            Fourcc::Abgr8888,
            (64, 64).into(),
        )
        .unwrap();
        let mut framebuffer = renderer.bind(&mut target).unwrap();
        OutputDamageTracker::new(viewport, 1.0, Transform::Normal)
            .render_output(
                &mut renderer,
                &mut framebuffer,
                0,
                &[element],
                Color32F::new(0.0, 0.0, 0.0, 0.0),
            )
            .unwrap();
        let mapping = renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size((64, 64).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let result = renderer.map_texture(&mapping).unwrap();
        let pixel = |x: usize, y: usize| &result[(y * 64 + x) * 4..(y * 64 + x + 1) * 4];
        for (x, y) in [(0, 0), (8, 8), (55, 8), (8, 55), (55, 55)] {
            assert_eq!(pixel(x, y)[3], 0, "blur outside rounded popup at {x},{y}");
        }
        assert_eq!(pixel(32, 32)[3], 255);
        assert!(pixel(31, 32)[0] > 30, "dark side of edge stays sharp");
        assert!(pixel(32, 32)[0] < 225, "light side of edge stays sharp");
    }
}
