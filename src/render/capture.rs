use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, Bind, ExportMem, Offscreen},
    },
    output::Output,
    utils::{Physical, Rectangle, Transform},
};

use crate::{
    render::{output_elements, CursorMode, Shaders, CLEAR_COLOR},
    state::BlairState,
};

/// Renders `output` offscreen and reads `region` back as tightly packed
/// `format` pixels.
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
    let elements = output_elements(renderer, state, output, shaders, cursor);

    let mut target = Offscreen::<smithay::backend::renderer::gles::GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        size.to_logical(1).to_buffer(1, Transform::Normal),
    )
    .context("failed to create the capture buffer")?;
    let mut framebuffer = renderer
        .bind(&mut target)
        .context("failed to bind the capture buffer")?;
    OutputDamageTracker::new(size, scale, Transform::Normal)
        .render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR)
        .map_err(|error| anyhow::anyhow!("failed to render the capture: {error}"))?;

    let region = Rectangle::new(
        (region.loc.x, region.loc.y).into(),
        (region.size.w, region.size.h).into(),
    );
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
