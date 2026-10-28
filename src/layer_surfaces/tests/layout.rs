use super::*;

fn resize(f: &mut Fixture, width: i32, height: i32) {
    f.output.change_current_state(
        Some(smithay::output::Mode {
            size: (width, height).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    f.state.output_changed(&f.output);
    f.dispatch();
}

#[test]
fn layer_sizes_must_fit_signed_coordinates() {
    for mapped in [false, true] {
        for (width, height) in [(u32::MAX, 30), (30, 1 << 31)] {
            let mut f = Fixture::new();
            if mapped {
                f.configure();
                f.map();
            }
            f.layer.set_size(width, height);
            f.surface.commit();
            let error = f.client.expect_protocol_error(&mut f.display, &mut f.state);
            assert_eq!(error.code, Error::InvalidSize as u32);
            assert_eq!(error.object_interface, "zwlr_layer_surface_v1");
        }
    }
    let mut f = Fixture::new();
    f.layer.set_size(i32::MAX as u32, i32::MAX as u32);
    f.surface.commit();
    f.dispatch();
    let (_, width, height) = *f.client.events.layer_configures.last().unwrap();
    assert_eq!((width, height), (800, 600));
    f.map();
    assert_eq!(f.area(), Rectangle::from_size((800, 600).into()));
}

#[test]
fn every_anchor_combination_survives_extreme_margins_and_zones() {
    let values = [i32::MIN, -i32::MAX, -100, 0, 100, i32::MAX];
    for bits in 0..16 {
        let mut f = Fixture::new();
        f.layer.set_size(10, 10);
        f.layer
            .set_anchor(zwlr_layer_surface_v1::Anchor::from_bits_retain(bits));
        f.layer.set_margin(i32::MIN, i32::MIN, i32::MIN, i32::MIN);
        f.surface.commit();
        f.dispatch();
        f.viewport.set_destination(10, 10);
        f.surface.attach(Some(&f.buffer), 0, 0);
        f.surface.commit();
        f.dispatch();
        for top_left in values {
            for bottom_right in values {
                f.layer
                    .set_margin(top_left, bottom_right, bottom_right, top_left);
                for zone in [-1, 0, 1, 30, i32::MAX] {
                    f.layer.set_exclusive_zone(zone);
                    f.surface.commit();
                    f.dispatch();
                    let area = f.area();
                    assert!(area.loc.x >= 0 && area.loc.y >= 0, "{bits}: {area:?}");
                    assert!(area.size.w >= 0 && area.size.h >= 0, "{bits}: {area:?}");
                    assert!(area.loc.x + area.size.w <= 800);
                    assert!(area.loc.y + area.size.h <= 600);
                    if zone <= 0 || (bits.count_ones() != 1 && bits.count_ones() != 3) {
                        assert_eq!(area, Rectangle::from_size((800, 600).into()));
                    }
                    for (_, width, height) in &f.client.events.layer_configures {
                        assert!(*width <= i32::MAX as u32 && *height <= i32::MAX as u32);
                    }
                    f.client.events.layer_configures.clear();
                }
            }
        }
    }
}

#[test]
fn negative_margins_preserve_positions_and_reduce_reservations() {
    let mut f = Fixture::new();
    f.settings();
    f.layer.set_margin(-10, -20, 0, -30);
    f.surface.commit();
    f.dispatch();
    assert_eq!(
        f.client
            .events
            .layer_configures
            .last()
            .map(|(_, w, h)| (*w, *h)),
        Some((850, 30))
    );
    f.map();
    assert_eq!(f.area(), Rectangle::new((0, 20).into(), (800, 580).into()));
    let map = layer_map_for_output(&f.output);
    let geometry = map.layer_geometry(map.layers().next().unwrap()).unwrap();
    assert_eq!(geometry.loc, (-30, -10).into());
}

#[test]
fn overlapping_reservations_recover_after_unmapping_and_output_changes() {
    let mut f = Fixture::new();
    f.settings();
    f.layer.set_exclusive_zone(500);
    f.surface.commit();
    f.dispatch();
    f.map();

    let surface = f
        .client
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&f.client.handle(), ());
    let layer = f
        .client
        .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4)
        .get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "second".into(),
            &f.client.handle(),
            (),
        );
    layer.set_size(10, 10);
    layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top);
    layer.set_exclusive_zone(500);
    surface.commit();
    f.dispatch();
    let _viewport = f.client.paint_surface(&surface, 10, 10);
    f.dispatch();
    assert_eq!(f.area(), Rectangle::new((0, 600).into(), (800, 0).into()));

    f.layer.set_exclusive_zone(i32::MAX);
    layer.set_exclusive_zone(i32::MAX);
    f.layer.set_margin(i32::MIN, i32::MIN, i32::MIN, i32::MIN);
    layer.set_margin(i32::MIN, 0, 0, 0);
    f.surface.commit();
    surface.commit();
    f.dispatch();
    assert_eq!(f.area(), Rectangle::new((0, 600).into(), (800, 0).into()));
    f.layer.set_margin(0, 0, 0, 0);
    layer.set_margin(0, 0, 0, 0);
    f.layer.set_exclusive_zone(500);
    layer.set_exclusive_zone(500);
    f.surface.commit();
    surface.commit();
    f.dispatch();

    f.unmap();
    assert_eq!(f.area(), Rectangle::new((0, 500).into(), (800, 100).into()));
    resize(&mut f, 800, 200);
    assert_eq!(f.area(), Rectangle::new((0, 200).into(), (800, 0).into()));

    layer.set_exclusive_zone(17);
    f.dispatch();
    resize(&mut f, 800, 1000);
    assert_eq!(f.area(), Rectangle::new((0, 500).into(), (800, 500).into()));
    surface.commit();
    f.dispatch();
    assert_eq!(f.area(), Rectangle::new((0, 17).into(), (800, 983).into()));
    layer.destroy();
    f.dispatch();
    assert_eq!(f.area(), Rectangle::from_size((800, 1000).into()));
}

#[test]
fn margins_can_reserve_the_entire_output_and_recover_after_resizing() {
    let mut f = Fixture::new();
    f.settings();
    f.layer.set_margin(1000, 0, 0, 0);
    f.surface.commit();
    f.dispatch();
    f.map();
    assert_eq!(f.area(), Rectangle::new((0, 600).into(), (800, 0).into()));
    resize(&mut f, 800, 1200);
    assert_eq!(
        f.area(),
        Rectangle::new((0, 1030).into(), (800, 170).into())
    );
    resize(&mut f, 800, 400);
    assert_eq!(f.area(), Rectangle::new((0, 400).into(), (800, 0).into()));
}

#[test]
fn fractional_output_sizes_use_the_same_rounding_before_and_after_mapping() {
    let mut f = Fixture::new();
    f.state.space.map_output(&f.output, (1500, -900));
    f.output.change_current_state(
        None,
        Some(smithay::utils::Transform::_90),
        Some(smithay::output::Scale::Fractional(1.7)),
        Some((1500, -900).into()),
    );
    f.state.output_changed(&f.output);
    f.configure();
    let (_, width, height) = *f.client.events.layer_configures.last().unwrap();
    assert_eq!((width, height), (353, 30));
    let count = f.client.events.layer_configures.len();
    f.map();
    assert_eq!(f.client.events.layer_configures.len(), count);
    assert_eq!(f.area(), Rectangle::new((0, 30).into(), (353, 441).into()));
}

#[test]
fn extreme_negative_margins_leave_room_for_global_output_coordinates() {
    let mut f = Fixture::new();
    f.state.space.map_output(&f.output, (1500, -900));
    f.layer.set_size(10, 10);
    f.layer.set_anchor(zwlr_layer_surface_v1::Anchor::all());
    f.layer.set_margin(i32::MIN, i32::MIN, i32::MIN, i32::MIN);
    f.surface.commit();
    f.dispatch();
    let (_, width, height) = *f.client.events.layer_configures.last().unwrap();
    assert_eq!(
        (width, height),
        ((i32::MAX - 1500) as u32, (i32::MAX - 900) as u32)
    );
    f.viewport.set_destination(10, 10);
    f.surface.attach(Some(&f.buffer), 0, 0);
    f.surface.commit();
    f.dispatch();
    let map = layer_map_for_output(&f.output);
    let geometry = map.layer_geometry(map.layers().next().unwrap()).unwrap();
    let global = geometry.loc + smithay::utils::Point::from((1500, -900));
    assert!(global.x.checked_add(width as i32).is_some());
    assert!(global.y.checked_add(height as i32).is_some());
}
