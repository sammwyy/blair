use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::utils::{Relocate, RelocateRenderElement},
            gles::{GlesRenderer, GlesTexture},
            Bind, ExportMem, Offscreen,
        },
    },
    output::Output,
    utils::{Physical, Rectangle, Transform},
};

use crate::{
    render::{output_elements, CursorMode, OutputRenderElement, Shaders, CLEAR_COLOR},
    state::BlairState,
};

pub(super) fn render_region(
    renderer: &mut GlesRenderer,
    elements: &mut [OutputRenderElement],
    region: Rectangle<i32, Physical>,
    scale: f64,
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
            .render_output(renderer, &mut framebuffer, 0, &relocated, CLEAR_COLOR)
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
