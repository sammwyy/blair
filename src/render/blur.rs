//! Renders the background blur `blair_blur_v1` requests (see [`crate::blur`]):
//! captures whatever is behind a blurred window into an offscreen texture,
//! then draws that texture back with a custom blur fragment shader, right
//! behind the window's own (typically translucent) content.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                texture::TextureRenderElement, Element, Id, Kind, RenderElement, UnderlyingStorage,
            },
            gles::{
                GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
                UniformName, UniformType, UniformValue,
            },
            utils::{CommitCounter, DamageSet, OpaqueRegions},
            Bind, Offscreen, Renderer,
        },
    },
    desktop::Window,
    output::Output,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform},
    wayland::shell::wlr_layer::Layer,
};

use super::{
    clip::local_from_ndc, layer_elements, window_elements, FrameContext, OutputRenderElement,
    RoundedClip, Shaders, CLEAR_COLOR,
};
use crate::state::BlairState;

const BLUR_SHADER: &str = r#"#version 100

precision highp float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_coords;

uniform vec2 texel;
uniform float spread;
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
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int x = -4; x <= 4; x++) {
        for (int y = -4; y <= 4; y++) {
            float fx = float(x);
            float fy = float(y);
            float weight = exp(-(fx * fx + fy * fy) / 18.0);
            sum += texture2D(tex, v_coords + vec2(fx, fy) * texel * spread) * weight;
            total += weight;
        }
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
            UniformName::new("clip_from_ndc", UniformType::Matrix3x3),
            UniformName::new("viewport", UniformType::_2f),
            UniformName::new("clip_size", UniformType::_2f),
            UniformName::new("clip_radii", UniformType::_4f),
        ],
    )
}

/// Captures `windows_behind` plus the bottom/background layers into an
/// offscreen texture, blurs `frame_rect` (output-logical) with a custom
/// shader, and pushes the result into `elements`. A no-op if the shader
/// failed to compile, `radius` is zero, or `frame_rect` falls outside the
/// output.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_backdrop(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    ctx: &FrameContext,
    shaders: Option<&Shaders>,
    font: &creamui_fonts::FontFace,
    windows_behind: &[Window],
    frame_rect: Rectangle<i32, Logical>,
    coverage: &[Rectangle<i32, Logical>],
    clip: Option<RoundedClip>,
    elements: &mut Vec<OutputRenderElement>,
) {
    let Some(shaders) = shaders else { return };
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

    let mut backdrop = Vec::new();
    for window in windows_behind.iter().rev() {
        window_elements(
            renderer,
            state,
            output,
            window,
            ctx,
            Some(shaders),
            font,
            &mut backdrop,
        );
    }
    layer_elements(
        renderer,
        state,
        output,
        ctx,
        Some(shaders),
        &[Layer::Bottom, Layer::Background],
        None,
        &mut backdrop,
    );

    let Ok(mut target) = Offscreen::<GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        ctx.viewport.to_logical(1).to_buffer(1, Transform::Normal),
    ) else {
        return;
    };
    {
        let Ok(mut framebuffer) = renderer.bind(&mut target) else {
            return;
        };
        if OutputDamageTracker::new(ctx.viewport, ctx.scale, Transform::Normal)
            .render_output(renderer, &mut framebuffer, 0, &backdrop, CLEAR_COLOR)
            .is_err()
        {
            return;
        }
    }

    let texel = (1.0 / ctx.viewport.w as f32, 1.0 / ctx.viewport.h as f32);
    let src = Rectangle::<f64, Logical>::new(
        Point::from((
            frame_rect_physical.loc.x as f64,
            frame_rect_physical.loc.y as f64,
        )),
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

pub struct BlurredBackdropElement {
    inner: TextureRenderElement<GlesTexture>,
    program: GlesTexProgram,
    texel: (f32, f32),
    spread: f32,
    coverage: Vec<Rectangle<i32, Physical>>,
    clip: RoundedClip,
}

impl BlurredBackdropElement {
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
            coverage,
            clip,
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
        let [tl, tr, br, bl] = self.clip.radii;
        let uniforms = vec![
            Uniform::new("texel", self.texel),
            Uniform::new("spread", self.spread),
            Uniform::new(
                "clip_from_ndc",
                UniformValue::Matrix3x3 {
                    matrices: vec![local_from_ndc(self.clip.rect, frame.projection())],
                    transpose: false,
                },
            ),
            Uniform::new(
                "viewport",
                (self.clip.viewport.w as f32, self.clip.viewport.h as f32),
            ),
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
mod tests {
    use super::*;
    use smithay::backend::{
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{Color32F, ExportMem, ImportMem},
    };

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn popup_blur_softens_the_backdrop_and_preserves_rounded_corners() {
        let device = EGLDevice::enumerate().unwrap().next().unwrap();
        let display = unsafe { EGLDisplay::new(device).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
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
