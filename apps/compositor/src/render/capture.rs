use std::path::Path;

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

/// Renders `output` and writes it as a PNG.
pub fn screenshot(
    renderer: &mut GlesRenderer,
    state: &mut BlairState,
    output: &Output,
    shaders: Option<&Shaders>,
    path: &Path,
) -> Result<()> {
    let size = output
        .current_mode()
        .context("the output has no current mode")?
        .size;
    let pixels = capture_region(
        renderer,
        state,
        output,
        shaders,
        Rectangle::from_size(size),
        CursorMode::Composited,
        Fourcc::Abgr8888,
    )?;
    write_png(path, size.w as u32, size.h as u32, &pixels)
}

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .context("failed to write the PNG header")?
        .write_image_data(pixels)
        .context("failed to write the PNG data")?;
    tracing::info!(path = %path.display(), width, height, "screenshot written");
    Ok(())
}
