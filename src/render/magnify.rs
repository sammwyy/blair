use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                texture::{TextureRenderBuffer, TextureRenderElement},
                Kind, RenderElementStates,
            },
            gles::{GlesRenderer, GlesTexture},
            Bind, ContextId, Offscreen, Renderer,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};

use super::{OutputRenderElement, CLEAR_COLOR};

pub fn source_rect(
    size: Size<i32, Physical>,
    pointer: Point<f64, Physical>,
    factor: f64,
) -> Rectangle<f64, Logical> {
    let factor = factor.clamp(1.0, 8.0);
    let offset = 1.0 - 1.0 / factor;
    Rectangle::new(
        (
            pointer.x.clamp(0.0, f64::from(size.w)) * offset,
            pointer.y.clamp(0.0, f64::from(size.h)) * offset,
        )
            .into(),
        (f64::from(size.w) / factor, f64::from(size.h) / factor).into(),
    )
}

pub struct Magnifier {
    context: ContextId<GlesTexture>,
    buffer: TextureRenderBuffer<GlesTexture>,
    tracker: OutputDamageTracker,
    size: Size<i32, Physical>,
    scale: f64,
    age: usize,
    pub states: RenderElementStates,
}

impl Magnifier {
    pub fn new(renderer: &mut GlesRenderer, size: Size<i32, Physical>, scale: f64) -> Result<Self> {
        let texture = Offscreen::<GlesTexture>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            (size.w, size.h).into(),
        )
        .context("create magnifier buffer")?;
        let opaque = Rectangle::from_size((size.w, size.h).into());
        Ok(Self {
            context: renderer.context_id(),
            buffer: TextureRenderBuffer::from_texture(
                renderer,
                texture,
                1,
                Transform::Normal,
                Some(vec![opaque]),
            ),
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            size,
            scale,
            age: 0,
            states: RenderElementStates::default(),
        })
    }

    pub fn matches(&self, renderer: &GlesRenderer, size: Size<i32, Physical>, scale: f64) -> bool {
        self.context == renderer.context_id() && self.size == size && self.scale == scale
    }

    pub fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputRenderElement],
        logical_size: Size<i32, Logical>,
        source: Rectangle<f64, Logical>,
    ) -> Result<TextureRenderElement<GlesTexture>> {
        let mut context = self.buffer.render();
        context.draw(|texture| -> Result<_> {
            let mut framebuffer = renderer.bind(texture).context("bind magnifier buffer")?;
            let frame = self
                .tracker
                .render_output(renderer, &mut framebuffer, self.age, elements, CLEAR_COLOR)
                .context("render magnifier scene")?;
            self.states = frame.states;
            let damage = frame
                .damage
                .into_iter()
                .flatten()
                .map(|rect| {
                    Rectangle::new(
                        (rect.loc.x, rect.loc.y).into(),
                        (rect.size.w, rect.size.h).into(),
                    )
                })
                .collect();
            Ok(damage)
        })?;
        drop(context);
        self.age = 1;
        Ok(TextureRenderElement::from_texture_render_buffer(
            (0.0, 0.0),
            &self.buffer,
            Some(1.0),
            Some(source),
            Some(logical_size),
            Kind::Unspecified,
        ))
    }
}

#[cfg(test)]
mod tests;
