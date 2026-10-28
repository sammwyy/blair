use smithay::{
    utils::{Logical, Rectangle, Size},
    wayland::shell::wlr_layer::{Anchor, ExclusiveZone, LayerSurfaceCachedState},
};

pub(super) struct Layout {
    output: Rectangle<i32, Logical>,
    pub(super) remaining: Rectangle<i32, Logical>,
    negative_budget: [i64; 2],
}

impl Layout {
    pub(super) fn new(output: Rectangle<i32, Logical>) -> Self {
        let local = Rectangle::from_size(output.size);
        Self {
            output: local,
            remaining: local,
            negative_budget: [
                negative_budget(output.size.w, output.loc.x),
                negative_budget(output.size.h, output.loc.y),
            ],
        }
    }

    pub(super) fn project(
        &mut self,
        mut data: LayerSurfaceCachedState,
    ) -> (LayerSurfaceCachedState, Size<i32, Logical>) {
        let source = if data.exclusive_zone == ExclusiveZone::DontCare {
            self.output
        } else {
            self.remaining
        };
        let mut available = source.size;
        let mut budget = self.negative_budget;
        bound_margin(
            data.anchor,
            Anchor::LEFT,
            &mut data.margin.left,
            &mut available.w,
            &mut budget[0],
        );
        bound_margin(
            data.anchor,
            Anchor::TOP,
            &mut data.margin.top,
            &mut available.h,
            &mut budget[1],
        );
        bound_margin(
            data.anchor,
            Anchor::RIGHT,
            &mut data.margin.right,
            &mut available.w,
            &mut budget[0],
        );
        bound_margin(
            data.anchor,
            Anchor::BOTTOM,
            &mut data.margin.bottom,
            &mut available.h,
            &mut budget[1],
        );

        let mut size = data.size;
        size.w = size.w.min(available.w);
        size.h = size.h.min(available.h);
        if size.w == 0 {
            size.w = available.w / 2;
        }
        if size.h == 0 {
            size.h = available.h / 2;
        }
        if data.anchor.anchored_horizontally() {
            size.w = available.w;
        }
        if data.anchor.anchored_vertically() {
            size.h = available.h;
        }

        if let ExclusiveZone::Exclusive(amount) = data.exclusive_zone {
            let edge = exclusive_edge(data.anchor);
            data.exclusive_zone = if let Some(edge) = edge {
                let (margin, extent, origin) = if edge == Anchor::LEFT || edge == Anchor::RIGHT {
                    (
                        if edge == Anchor::LEFT {
                            data.margin.left
                        } else {
                            data.margin.right
                        },
                        &mut self.remaining.size.w,
                        &mut self.remaining.loc.x,
                    )
                } else {
                    (
                        if edge == Anchor::TOP {
                            data.margin.top
                        } else {
                            data.margin.bottom
                        },
                        &mut self.remaining.size.h,
                        &mut self.remaining.loc.y,
                    )
                };
                let reservation =
                    (i64::from(amount) + i64::from(margin)).clamp(0, i64::from(*extent)) as i32;
                let amount = (i64::from(reservation) - i64::from(margin)) as u32;
                *extent -= reservation;
                if edge == Anchor::LEFT || edge == Anchor::TOP {
                    *origin += reservation;
                }
                // A zero amount can still reserve the margin in Smithay.
                ExclusiveZone::Exclusive(amount)
            } else {
                ExclusiveZone::Neutral
            };
        }
        (data, size)
    }
}

// Geometry must remain representable after global translation or removal of earlier reservations.
fn negative_budget(extent: i32, origin: i32) -> i64 {
    (i64::from(i32::MAX) - i64::from(extent) - i64::from(origin).abs()).max(0)
}

fn bound_margin(
    anchors: Anchor,
    edge: Anchor,
    margin: &mut i32,
    extent: &mut i32,
    budget: &mut i64,
) {
    if !anchors.contains(edge) {
        return;
    }
    let bounded = i64::from(*margin).clamp(-*budget, i64::from(*extent));
    *margin = bounded as i32;
    *extent = (i64::from(*extent) - bounded) as i32;
    *budget += bounded.min(0);
}

fn exclusive_edge(anchor: Anchor) -> Option<Anchor> {
    [Anchor::LEFT, Anchor::RIGHT, Anchor::TOP, Anchor::BOTTOM]
        .into_iter()
        .find(|edge| {
            let perpendicular = if *edge == Anchor::LEFT || *edge == Anchor::RIGHT {
                Anchor::TOP | Anchor::BOTTOM
            } else {
                Anchor::LEFT | Anchor::RIGHT
            };
            anchor == *edge || anchor == *edge | perpendicular
        })
}
