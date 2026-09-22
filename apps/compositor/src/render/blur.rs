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
                UniformName, UniformType,
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
    layer_elements, window_elements, FrameContext, OutputRenderElement, Shaders, CLEAR_COLOR,
};
use crate::state::BlairState;

const BLUR_SHADER: &str = r#"#version 100

precision highp float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_coords;

uniform vec2 texel;
uniform float spread;

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
    gl_FragColor = (sum / total) * alpha;
}
"#;

pub fn compile(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    renderer.compile_custom_texture_shader(
        BLUR_SHADER,
        &[
            UniformName::new("texel", UniformType::_2f),
            UniformName::new("spread", UniformType::_1f),
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
    font: &fontdue::Font,
    windows_behind: &[Window],
    frame_rect: Rectangle<i32, Logical>,
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
    elements.push(BlurredBackdropElement::new(inner, shaders.blur.clone(), texel, radius).into());
}

pub struct BlurredBackdropElement {
    inner: TextureRenderElement<GlesTexture>,
    program: GlesTexProgram,
    texel: (f32, f32),
    spread: f32,
}

impl BlurredBackdropElement {
    fn new(
        inner: TextureRenderElement<GlesTexture>,
        program: GlesTexProgram,
        texel: (f32, f32),
        spread: f32,
    ) -> Self {
        Self {
            inner,
            program,
            texel,
            spread,
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
        let uniforms = vec![
            Uniform::new("texel", self.texel),
            Uniform::new("spread", self.spread),
        ];
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let result = RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
        );
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
