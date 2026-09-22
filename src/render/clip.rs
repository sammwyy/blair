use smithay::{
    backend::renderer::{
        element::{
            surface::WaylandSurfaceRenderElement, Element, Id, Kind, RenderElement,
            UnderlyingStorage,
        },
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
            UniformValue,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
};

// Coverage is computed from gl_FragCoord rather than v_coords, so the clip
// is independent of buffer transform, y-inversion and src cropping.
const CLIP_SHADER: &str = r#"#version 100

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
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

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
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

    vec2 ndc = gl_FragCoord.xy / viewport * 2.0 - 1.0;
    color *= coverage((clip_from_ndc * vec3(ndc, 1.0)).xy);

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

pub fn compile(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    renderer.compile_custom_texture_shader(
        CLIP_SHADER,
        &[
            UniformName::new("clip_from_ndc", UniformType::Matrix3x3),
            UniformName::new("viewport", UniformType::_2f),
            UniformName::new("clip_size", UniformType::_2f),
            UniformName::new("clip_radii", UniformType::_4f),
        ],
    )
}

/// Rounded clip in output physical coordinates. Radii are ordered
/// top-left, top-right, bottom-right, bottom-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundedClip {
    pub rect: Rectangle<i32, Physical>,
    pub radii: [f32; 4],
    pub viewport: Size<i32, Physical>,
}

impl RoundedClip {
    fn corner_squares(&self) -> [Rectangle<i32, Physical>; 4] {
        let r = self.radii.map(|radius| radius.ceil() as i32);
        let Rectangle { loc, size } = self.rect;
        [
            Rectangle::new(loc, (r[0], r[0]).into()),
            Rectangle::new((loc.x + size.w - r[1], loc.y).into(), (r[1], r[1]).into()),
            Rectangle::new(
                (loc.x + size.w - r[2], loc.y + size.h - r[2]).into(),
                (r[2], r[2]).into(),
            ),
            Rectangle::new((loc.x, loc.y + size.h - r[3]).into(), (r[3], r[3]).into()),
        ]
    }

    /// Whether drawing `geometry` unclipped would differ from drawing it
    /// through this clip.
    pub fn affects(&self, geometry: Rectangle<i32, Physical>) -> bool {
        if self.rect.intersection(geometry) != Some(geometry) {
            return true;
        }
        self.corner_squares()
            .iter()
            .any(|corner| !corner.is_empty() && corner.overlaps(geometry))
    }
}

pub(crate) fn local_from_ndc(rect: Rectangle<i32, Physical>, projection: &[f32; 9]) -> [f32; 9] {
    let local_from_output = [
        1.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        -rect.loc.x as f32,
        -rect.loc.y as f32,
        1.0,
    ];
    mat3_mul(&local_from_output, &mat3_inverse(projection))
}

pub struct ClippedSurfaceElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    clip: RoundedClip,
}

impl ClippedSurfaceElement {
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        clip: RoundedClip,
    ) -> Self {
        Self {
            inner,
            program,
            clip,
        }
    }

    fn uniforms(&self, projection: &[f32; 9]) -> Vec<Uniform<'static>> {
        let clip_from_ndc = local_from_ndc(self.clip.rect, projection);
        let [tl, tr, br, bl] = self.clip.radii;
        vec![
            Uniform::new(
                "clip_from_ndc",
                UniformValue::Matrix3x3 {
                    matrices: vec![clip_from_ndc],
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
        ]
    }
}

impl Element for ClippedSurfaceElement {
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

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let geometry = self.inner.geometry(scale);
        let Some(visible) = self.clip.rect.intersection(geometry) else {
            return OpaqueRegions::default();
        };
        let to_local =
            |rect: Rectangle<i32, Physical>| Rectangle::new(rect.loc - geometry.loc, rect.size);
        let visible = to_local(visible);
        let corners = self
            .clip
            .corner_squares()
            .into_iter()
            .filter(|corner| !corner.is_empty())
            .map(to_local);
        let regions = self
            .inner
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|region| region.intersection(visible));
        Rectangle::subtract_rects_many(regions, corners)
            .into_iter()
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for ClippedSurfaceElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let uniforms = self.uniforms(frame.projection());
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let result = self.inner.draw(frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

fn mat3_mul(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for col in 0..3 {
        for row in 0..3 {
            out[col * 3 + row] = (0..3).map(|k| a[k * 3 + row] * b[col * 3 + k]).sum();
        }
    }
    out
}

/// Inverse of a column-major 3x3 matrix.
fn mat3_inverse(m: &[f32; 9]) -> [f32; 9] {
    let at = |row: usize, col: usize| f64::from(m[col * 3 + row]);
    let cofactor = |row: usize, col: usize| {
        let (r1, r2) = ((row + 1) % 3, (row + 2) % 3);
        let (c1, c2) = ((col + 1) % 3, (col + 2) % 3);
        at(r1, c1) * at(r2, c2) - at(r1, c2) * at(r2, c1)
    };
    let determinant: f64 = (0..3).map(|col| at(0, col) * cofactor(0, col)).sum();
    if determinant.abs() < f64::EPSILON {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let mut inverse = [0.0; 9];
    for col in 0..3 {
        for row in 0..3 {
            inverse[col * 3 + row] = (cofactor(col, row) / determinant) as f32;
        }
    }
    inverse
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &[f32; 9], x: f32, y: f32) -> (f32, f32) {
        (m[0] * x + m[3] * y + m[6], m[1] * x + m[4] * y + m[7])
    }

    #[test]
    fn inverse_round_trips_an_affine_projection() {
        let projection = [
            2.0 / 1920.0,
            0.0,
            0.0,
            0.0,
            -2.0 / 1080.0,
            0.0,
            -1.0,
            1.0,
            1.0,
        ];
        let inverse = mat3_inverse(&projection);
        let (nx, ny) = apply(&projection, 480.0, 270.0);
        let (x, y) = apply(&inverse, nx, ny);
        assert!((x - 480.0).abs() < 1e-3 && (y - 270.0).abs() < 1e-3);
        let identity = mat3_mul(&projection, &inverse);
        for (index, value) in identity.iter().enumerate() {
            let expected = if index % 4 == 0 { 1.0 } else { 0.0 };
            assert!((value - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn clip_only_affects_geometry_touching_corners_or_edges() {
        let clip = RoundedClip {
            rect: Rectangle::new((100, 100).into(), (400, 300).into()),
            radii: [0.0, 0.0, 12.0, 12.0],
            viewport: (1920, 1080).into(),
        };
        assert!(!clip.affects(Rectangle::new((100, 100).into(), (50, 50).into())));
        assert!(clip.affects(Rectangle::new((450, 350).into(), (50, 50).into())));
        assert!(clip.affects(Rectangle::new((90, 150).into(), (50, 50).into())));
        assert!(!clip.affects(Rectangle::new((150, 150).into(), (100, 100).into())));
    }
}
