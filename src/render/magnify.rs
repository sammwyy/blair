use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{texture::TextureRenderElement, Id, Kind, RenderElementStates},
            gles::{GlesRenderer, GlesTexture},
            Bind, Offscreen, Renderer,
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

pub fn render(
    renderer: &mut GlesRenderer,
    elements: &[OutputRenderElement],
    size: Size<i32, Physical>,
    logical_size: Size<i32, Logical>,
    scale: f64,
    source: Rectangle<f64, Logical>,
) -> Result<(TextureRenderElement<GlesTexture>, RenderElementStates)> {
    let mut texture = Offscreen::<GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        (size.w, size.h).into(),
    )
    .context("create magnifier buffer")?;
    let mut framebuffer = renderer
        .bind(&mut texture)
        .context("bind magnifier buffer")?;
    let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
    let frame = tracker
        .render_output(renderer, &mut framebuffer, 0, elements, CLEAR_COLOR)
        .context("render magnifier scene")?;
    frame.sync.wait().context("synchronize magnifier scene")?;
    let states = frame.states;
    drop(framebuffer);
    let element = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture,
        1,
        Transform::Normal,
        Some(1.0),
        Some(source),
        Some(logical_size),
        None,
        Kind::Unspecified,
    );
    Ok((element, states))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnification_keeps_the_pointer_anchored_and_the_source_inside_the_output() {
        let size = Size::from((1000, 600));
        for pointer in [
            Point::from((0.0, 0.0)),
            (500.0, 300.0).into(),
            (999.0, 599.0).into(),
        ] {
            let source = source_rect(size, pointer, 2.0);
            assert_eq!((pointer.x - source.loc.x) * 2.0, pointer.x);
            assert_eq!((pointer.y - source.loc.y) * 2.0, pointer.y);
            assert!(source.loc.x >= 0.0 && source.loc.x + source.size.w <= 1000.0);
            assert!(source.loc.y >= 0.0 && source.loc.y + source.size.h <= 600.0);
        }
    }

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn magnifier_renders_the_selected_region_at_output_size() {
        use smithay::backend::{
            egl::{EGLContext, EGLDevice, EGLDisplay},
            renderer::{element::solid::SolidColorRenderElement, Color32F, ExportMem},
        };
        let device = EGLDevice::enumerate().unwrap().next().unwrap();
        let display = unsafe { EGLDisplay::new(device).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
        let size = Size::from((64, 64));
        let colors = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ];
        let elements: Vec<_> = colors
            .into_iter()
            .enumerate()
            .map(|(index, color)| {
                SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(
                        (((index % 2) * 32) as i32, ((index / 2) * 32) as i32).into(),
                        (32, 32).into(),
                    ),
                    smithay::backend::renderer::utils::CommitCounter::default(),
                    color,
                    Kind::Unspecified,
                )
                .into()
            })
            .collect();
        let source = source_rect(size, (0.0, 0.0).into(), 2.0);
        let (element, _) =
            render(&mut renderer, &elements, size, (64, 64).into(), 1.0, source).unwrap();
        let mut target = Offscreen::<GlesTexture>::create_buffer(
            &mut renderer,
            Fourcc::Abgr8888,
            (64, 64).into(),
        )
        .unwrap();
        let mut framebuffer = renderer.bind(&mut target).unwrap();
        OutputDamageTracker::new(size, 1.0, Transform::Normal)
            .render_output(
                &mut renderer,
                &mut framebuffer,
                0,
                &[element],
                Color32F::new(0.0, 0.0, 0.0, 1.0),
            )
            .unwrap()
            .sync
            .wait()
            .unwrap();
        let mapping = renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size((size.w, size.h).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        for (x, y) in [(8, 8), (56, 8), (8, 56), (56, 56)] {
            assert_eq!(
                &pixels[(y * 64 + x) * 4..(y * 64 + x + 1) * 4],
                &[255, 0, 0, 255]
            );
        }
    }
}
