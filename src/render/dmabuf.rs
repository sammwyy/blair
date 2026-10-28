use smithay::{
    backend::{
        allocator::format::FormatSet,
        renderer::element::{RenderElementPresentationState, RenderElementStates, RenderingReason},
    },
    desktop::utils::{surface_primary_scanout_output, with_surfaces_surface_tree},
    output::Output,
    reexports::{
        rustix::fs::Dev,
        wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags,
    },
    wayland::{
        dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, SurfaceDmabufFeedbackState},
        seat::WaylandFocus,
    },
};

use crate::state::BlairState;

pub struct ScanoutFeedback {
    render: DmabufFeedback,
    scanout: DmabufFeedback,
}

impl ScanoutFeedback {
    pub fn new(
        render_device: Dev,
        scanout_device: Dev,
        render_formats: &FormatSet,
        plane_formats: &FormatSet,
    ) -> std::io::Result<Self> {
        let render =
            DmabufFeedbackBuilder::new(render_device, render_formats.iter().copied()).build()?;
        let mut shared = render_formats
            .intersection(plane_formats)
            .copied()
            .peekable();
        let scanout = if shared.peek().is_some() {
            DmabufFeedbackBuilder::new(render_device, render_formats.iter().copied())
                .add_preference_tranche(scanout_device, Some(TrancheFlags::Scanout), shared)
                .build()?
        } else {
            render.clone()
        };
        Ok(Self { render, scanout })
    }

    pub fn render(&self) -> &DmabufFeedback {
        &self.render
    }

    pub fn send(&self, state: &BlairState, output: &Output, render_states: &RenderElementStates) {
        let effects = state.session_locked
            || (state.config.accessibility.zoom && state.config.accessibility.zoom_factor > 1.0);
        super::for_each_surface_root(state, |root| {
            let fullscreen = !effects
                && state.windows.values().any(|managed| {
                    managed.fullscreen
                        && managed.window.wl_surface().as_deref() == Some(root)
                        && state.window_visible_on_output(&managed.window, output)
                });
            with_surfaces_surface_tree(root, |surface, states| {
                let primary = surface_primary_scanout_output(surface, states);
                if primary.as_ref().is_some_and(|primary| primary != output) {
                    return;
                }
                let Some(feedback) = SurfaceDmabufFeedbackState::from_states(states) else {
                    return;
                };
                let presentation = render_states
                    .element_render_state(surface)
                    .filter(|state| state.visible_area > 0)
                    .map(|state| state.presentation_state);
                let scanout =
                    primary.as_ref() == Some(output) && fullscreen && prefers_scanout(presentation);
                feedback.set_feedback(if scanout { &self.scanout } else { &self.render });
            });
        });
    }
}

fn prefers_scanout(presentation: Option<RenderElementPresentationState>) -> bool {
    matches!(
        presentation,
        Some(
            RenderElementPresentationState::ZeroCopy
                | RenderElementPresentationState::Rendering {
                    reason: Some(RenderingReason::FormatUnsupported)
                }
        )
    )
}

#[cfg(test)]
mod tests;
