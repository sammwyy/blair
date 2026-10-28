use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use smithay::{
    desktop::utils::{surface_primary_scanout_output, with_surfaces_surface_tree},
    output::Output,
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        RegistrationToken,
    },
    wayland::compositor::{SurfaceAttributes, SurfaceData},
};

use crate::state::BlairState;

use super::for_each_surface_root;

const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_millis(995);

struct CallbackDeadline(Cell<Instant>);

#[derive(Default)]
pub(crate) struct HiddenFrames {
    timer: Option<(RegistrationToken, Instant)>,
}

fn complete(states: &SurfaceData, now: Instant, time: Duration) {
    let mut cache = states.cached_state.get::<SurfaceAttributes>();
    let callbacks = &mut cache.current().frame_callbacks;
    if callbacks.is_empty() {
        return;
    }
    states
        .data_map
        .get_or_insert(|| CallbackDeadline(Cell::new(now)))
        .0
        .set(now + HIDDEN_FRAME_INTERVAL);
    for callback in callbacks.drain(..) {
        callback.done(time.as_millis() as u32);
    }
}

fn hidden_deadline(state: &BlairState, now: Instant, send: bool) -> Option<Instant> {
    let mut next = None;
    let time = state.clock_now();
    for_each_surface_root(state, |root| {
        with_surfaces_surface_tree(root, |surface, states| {
            if states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .is_empty()
                || surface_primary_scanout_output(surface, states)
                    .is_some_and(|output| state.space.outputs().any(|mapped| *mapped == output))
            {
                return;
            }
            let deadline = states
                .data_map
                .get_or_insert(|| CallbackDeadline(Cell::new(now + HIDDEN_FRAME_INTERVAL)))
                .0
                .get();
            if send && deadline <= now {
                complete(states, now, time);
            } else {
                next = Some(next.map_or(deadline, |previous: Instant| previous.min(deadline)));
            }
        });
    });
    next
}

pub(crate) fn send_frame_callbacks(state: &mut BlairState, output: &Output, time: Duration) {
    profiling::scope!("send_frame_callbacks");
    let now = Instant::now();
    for_each_surface_root(state, |root| {
        with_surfaces_surface_tree(root, |surface, states| {
            if surface_primary_scanout_output(surface, states).as_ref() == Some(output) {
                complete(states, now, time);
            }
        });
    });
    state.schedule_hidden_frames();
}

impl BlairState {
    pub(crate) fn schedule_hidden_frames(&mut self) {
        let deadline = hidden_deadline(self, Instant::now(), false);
        if let Some((token, current)) = self.hidden_frames.timer {
            if deadline.is_some_and(|deadline| current <= deadline) {
                return;
            }
            self.loop_handle.remove(token);
            self.hidden_frames.timer = None;
        }
        let Some(deadline) = deadline else {
            return;
        };
        match self
            .loop_handle
            .insert_source(Timer::from_deadline(deadline), |_, _, state| {
                if let Some(deadline) = hidden_deadline(state, Instant::now(), true) {
                    state.hidden_frames.timer.as_mut().unwrap().1 = deadline;
                    TimeoutAction::ToInstant(deadline)
                } else {
                    state.hidden_frames.timer = None;
                    TimeoutAction::Drop
                }
            }) {
            Ok(token) => self.hidden_frames.timer = Some((token, deadline)),
            Err(error) => tracing::warn!(%error, "could not schedule hidden frame callbacks"),
        }
    }
}

#[cfg(test)]
mod tests;
