use smithay::{
    backend::renderer::{
        element::{solid::SolidColorBuffer, Element, Id, Kind, RenderElement},
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType,
        },
        utils::CommitCounter,
        Color32F,
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform},
};

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

/// Logical-unit frame parameters; colors are straight (non-premultiplied).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameParams {
    pub radius: f32,
    pub border_width: f32,
    pub titlebar_height: f32,
    pub border_color: Color32F,
    pub titlebar_color: Color32F,
}

/// Per-window decoration state kept across frames so render element ids
/// and commit counters stay stable and damage tracking only repaints what
/// changed.
#[derive(Debug)]
pub struct WindowDecoration {
    id: Id,
    commit: CommitCounter,
    area: Rectangle<i32, Logical>,
    params: Option<FrameParams>,
    pub buttons: [SolidColorBuffer; 3],
}

impl Default for WindowDecoration {
    fn default() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            area: Rectangle::default(),
            params: None,
            buttons: Default::default(),
        }
    }
}

impl WindowDecoration {
    pub fn update(&mut self, area: Rectangle<i32, Logical>, params: FrameParams) {
        if self.area != area || self.params != Some(params) {
            self.area = area;
            self.params = Some(params);
            self.commit.increment();
        }
    }

    pub fn element(
        &self,
        program: &GlesPixelProgram,
        output_origin: smithay::utils::Point<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<FrameElement> {
        let params = self.params?;
        Some(FrameElement {
            id: self.id.clone(),
            commit: self.commit,
            geometry: Rectangle::new(self.area.loc - output_origin, self.area.size)
                .to_physical_precise_round(scale),
            scale,
            params,
            alpha,
            program: program.clone(),
        })
    }
}

pub struct FrameElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    scale: f64,
    params: FrameParams,
    alpha: f32,
    program: GlesPixelProgram,
}

impl Element for FrameElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
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
        )
    }
}

fn premultiplied(color: Color32F) -> (f32, f32, f32, f32) {
    let a = color.a();
    (color.r() * a, color.g() * a, color.b() * a, a)
}
