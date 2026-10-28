use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                utils::{Relocate, RelocateRenderElement},
                Element,
            },
            gles::{GlesMapping, GlesRenderer, GlesTarget, GlesTexture},
            Bind, Color32F, ExportMem, Offscreen,
        },
    },
    output::Output,
    utils::{Physical, Rectangle, Size, Transform},
};

pub fn capture_cursor(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
) -> Result<Option<(Rectangle<i32, Physical>, GlesMapping)>> {
    let Some(output_geo) = state.space.output_geometry(output) else {
        return Ok(None);
    };
    let viewport = output.current_transform().transform_size(
        output
            .current_mode()
            .context("the output has no current mode")?
            .size,
    );
    let scale = output.current_scale().fractional_scale();
    let ctx = super::FrameContext {
        output_geo,
        scale,
        viewport,
    };
    let mut elements = Vec::new();
    super::cursor_elements(
        renderer,
        state,
        output,
        &ctx,
        CursorMode::Composited,
        &mut elements,
    );
    let Some(region) = elements
        .iter()
        .map(|element| element.geometry(scale.into()))
        .reduce(|a, b| a.merge(b))
        .and_then(|region| region.intersection(Rectangle::from_size(viewport)))
    else {
        return Ok(None);
    };
    let mut texture = render_region_with_clear(
        renderer,
        &mut elements,
        region,
        scale,
        Color32F::new(0.0, 0.0, 0.0, 0.0),
    )?;
    let framebuffer = renderer.bind(&mut texture).context("bind cursor capture")?;
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(region.size.to_logical(1).to_buffer(1, Transform::Normal)),
            Fourcc::Argb8888,
        )
        .context("read cursor capture")?;
    Ok(Some((region, mapping)))
}

use crate::{
    render::{output_elements, CursorMode, OutputRenderElement, Shaders, CLEAR_COLOR},
    state::BlairState,
};

pub fn read_framebuffer(
    renderer: &mut GlesRenderer,
    framebuffer: &GlesTarget<'_>,
    size: Size<i32, Physical>,
    region: Rectangle<i32, Physical>,
    transform: Transform,
) -> Result<GlesMapping> {
    anyhow::ensure!(
        !region.is_empty() && region.intersection(Rectangle::from_size(size)) == Some(region),
        "the capture region is outside the framebuffer"
    );
    let region = transform
        .transform_rect_in(region, &size)
        .to_logical(1)
        .to_buffer(
            1,
            Transform::Normal,
            &transform.transform_size(size).to_logical(1),
        );
    renderer
        .copy_framebuffer(framebuffer, region, Fourcc::Xrgb8888)
        .context("failed to read the output framebuffer")
}

pub(super) fn render_region(
    renderer: &mut GlesRenderer,
    elements: &mut [OutputRenderElement],
    region: Rectangle<i32, Physical>,
    scale: f64,
) -> Result<GlesTexture> {
    render_region_with_clear(renderer, elements, region, scale, CLEAR_COLOR)
}

fn render_region_with_clear(
    renderer: &mut GlesRenderer,
    elements: &mut [OutputRenderElement],
    region: Rectangle<i32, Physical>,
    scale: f64,
    clear: Color32F,
) -> Result<GlesTexture> {
    for element in elements.iter_mut() {
        match element {
            OutputRenderElement::Clipped(element) => element.set_offscreen_region(Some(region)),
            OutputRenderElement::Blur(element) => element.set_offscreen_region(Some(region)),
            _ => {}
        }
    }
    let result = (|| {
        let relocated: Vec<_> = elements
            .iter()
            .map(|element| {
                RelocateRenderElement::from_element(
                    element,
                    (-region.loc.x, -region.loc.y),
                    Relocate::Relative,
                )
            })
            .collect();
        let mut target = Offscreen::<GlesTexture>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            region.size.to_logical(1).to_buffer(1, Transform::Normal),
        )
        .context("failed to create the offscreen region buffer")?;
        let mut framebuffer = renderer
            .bind(&mut target)
            .context("failed to bind the offscreen region buffer")?;
        OutputDamageTracker::new(region.size, scale, Transform::Normal)
            .render_output(renderer, &mut framebuffer, 0, &relocated, clear)
            .map_err(|error| anyhow::anyhow!("failed to render the offscreen region: {error}"))?;
        drop(framebuffer);
        Ok(target)
    })();
    for element in elements {
        match element {
            OutputRenderElement::Clipped(element) => element.set_offscreen_region(None),
            OutputRenderElement::Blur(element) => element.set_offscreen_region(None),
            _ => {}
        }
    }
    result
}

/// Renders `region` offscreen and reads it back as tightly packed `format` pixels.
pub fn capture_region(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    shaders: Option<&Shaders>,
    region: Rectangle<i32, Physical>,
    cursor: CursorMode,
    format: Fourcc,
) -> Result<Vec<u8>> {
    profiling::scope!("capture_region");
    let size = output
        .current_mode()
        .context("the output has no current mode")?
        .size;
    let scale = output.current_scale().fractional_scale();
    anyhow::ensure!(
        !region.is_empty() && region.intersection(Rectangle::from_size(size)) == Some(region),
        "the capture region is outside the output"
    );
    let mut elements = output_elements(renderer, state, output, shaders, cursor);
    let mut target = render_region(renderer, &mut elements, region, scale)?;
    let framebuffer = renderer
        .bind(&mut target)
        .context("failed to bind the capture buffer")?;
    let region = Rectangle::from_size(region.size.to_logical(1).to_buffer(1, Transform::Normal));
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, format)
        .context("failed to copy the framebuffer")?;
    let pixels = renderer
        .map_texture(&mapping)
        .context("failed to map the capture")?
        .to_vec();
    drop(framebuffer);
    Ok(pixels)
}
