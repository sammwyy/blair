use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, UniformName, UniformType,
};

const MASK_FRAGMENT_SHADER: &str = "
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float radius;

void main() {
    vec2 pos = v_coords * size;
    vec2 inner = clamp(pos, vec2(radius), size - vec2(radius));
    if (distance(pos, inner) > radius) {
        discard;
    }
    gl_FragColor = vec4(1.0);
}
";

const RING_FRAGMENT_SHADER: &str = "
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform float radius;
uniform float inner_radius;
uniform vec4 color;

void main() {
    vec2 pos = v_coords * size;
    vec2 pivot = clamp(pos, vec2(radius), size - vec2(radius));
    float dist = distance(pos, pivot);
    float outer = 1.0 - smoothstep(radius - 1.0, radius + 1.0, dist);
    float inner = smoothstep(inner_radius - 1.0, inner_radius + 1.0, dist);
    gl_FragColor = vec4(color.rgb, 1.0) * (outer * inner * color.a * alpha);
}
";

/// Programs used to give a window's frame rounded corners.
#[derive(Clone)]
pub struct RoundedCornerShaders {
    /// Writes to the stencil buffer wherever a fragment is inside the outer
    /// radius, so content and decoration can be clipped to it. Fragments
    /// outside are discarded, never touching the stencil buffer — leaving
    /// whatever was drawn earlier (background, layers, windows below)
    /// untouched at the corners.
    pub mask: GlesPixelProgram,
    /// Paints the themed border as a rounded ring between an outer and
    /// inner radius, following the same curve as the mask on the outside
    /// while leaving the interior (content) untouched.
    pub ring: GlesPixelProgram,
}

pub fn compile(renderer: &mut GlesRenderer) -> Result<RoundedCornerShaders, GlesError> {
    let mask = renderer.compile_custom_pixel_shader(
        MASK_FRAGMENT_SHADER,
        &[UniformName::new("radius", UniformType::_1f)],
    )?;
    let ring = renderer.compile_custom_pixel_shader(
        RING_FRAGMENT_SHADER,
        &[
            UniformName::new("radius", UniformType::_1f),
            UniformName::new("inner_radius", UniformType::_1f),
            UniformName::new("color", UniformType::_4f),
        ],
    )?;
    Ok(RoundedCornerShaders { mask, ring })
}
