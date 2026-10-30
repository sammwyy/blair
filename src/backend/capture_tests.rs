use super::*;
use crate::{
    render::CursorMode,
    test_support::{ClientEvents, EglTest, TestClient},
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{texture::TextureRenderElement, Id, Kind},
            gles::GlesTexture,
            Bind, Color32F, ExportMem, ImportMem, Offscreen, Renderer,
        },
    },
    reexports::rustix::fs::{memfd_create, MemfdFlags},
    utils::{Rectangle, Transform},
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsFd,
};
use wayland_client::{
    protocol::{wl_output::WlOutput, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

wayland_client::delegate_noop!(ClientEvents: ignore WlShm);
wayland_client::delegate_noop!(ClientEvents: ignore WlShmPool);
wayland_client::delegate_noop!(ClientEvents: ignore ZwlrScreencopyManagerV1);

impl Dispatch<ZwlrScreencopyFrameV1, u32> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        label: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.screencopy.push((*label, event));
    }
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn protocol_captures_reuse_the_frame_and_preserve_cursor_fallbacks() {
    let egl = EglTest::new();
    let mut renderer = egl.renderer();
    let (mut display, _event_loop, mut state) = crate::state::tests::fixture();
    state.config.integrations.screencopy = crate::config::ScreencopyPolicy::Allow;
    state.config.accessibility.zoom = true;
    state.config.accessibility.zoom_factor = 2.0;
    let output = crate::test_support::add_output(&mut state, "capture", (0, 0), (8, 6));
    let mut client = TestClient::new(&mut display, &mut state);
    let manager = client.bind::<ZwlrScreencopyManagerV1>(3);
    let client_output = client.bind::<WlOutput>(4);
    let mut file = File::from(memfd_create("blair-screencopy-test", MemfdFlags::CLOEXEC).unwrap());
    let stride = 36;
    let offset = 64;
    let sentinel = vec![0xab; offset + stride * 6 + 16];
    file.write_all(&sentinel).unwrap();
    let pool = client.bind::<WlShm>(1).create_pool(
        file.as_fd(),
        sentinel.len() as i32,
        &client.handle(),
        (),
    );
    let buffer = pool.create_buffer(
        offset as i32,
        8,
        6,
        stride as i32,
        wl_shm::Format::Xrgb8888,
        &client.handle(),
        (),
    );
    for label in 0..3 {
        let frame = manager.capture_output(
            u32::from(label == 2) as i32,
            &client_output,
            &client.handle(),
            label,
        );
        client.dispatch(&mut display, &mut state);
        if label == 1 {
            frame.copy_with_damage(&buffer);
        } else {
            frame.copy(&buffer);
        }
    }
    client.dispatch(&mut display, &mut state);
    assert_eq!(state.pending_captures.len(), 3);
    let pixels: Vec<u8> = (0..6)
        .flat_map(|y| (0..8).flat_map(move |x| [x * 23, y * 31, 100, 255]))
        .collect();
    let texture = renderer
        .import_memory(&pixels, Fourcc::Abgr8888, (8, 6).into(), false)
        .unwrap();
    let element = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture.clone(),
        1,
        Transform::Normal,
        None,
        None,
        None,
        None,
        Kind::Unspecified,
    );
    let mut target =
        Offscreen::<GlesTexture>::create_buffer(&mut renderer, Fourcc::Abgr8888, (8, 6).into())
            .unwrap();
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    OutputDamageTracker::new((8, 6), 1.0, Transform::Flipped180)
        .render_output(
            &mut renderer,
            &mut framebuffer,
            0,
            &[element],
            Color32F::BLACK,
        )
        .unwrap();
    process_frame_captures(
        &mut state,
        &mut renderer,
        &output,
        &framebuffer,
        Transform::Flipped180,
        CursorMode::HostNamed,
    );
    drop(framebuffer);
    assert_eq!(state.pending_captures.len(), 1);
    assert!(state.pending_captures[0].overlay_cursor);
    let mut copied = vec![0; sentinel.len()];
    file.seek(SeekFrom::Start(0)).unwrap();
    file.read_exact(&mut copied).unwrap();
    assert_eq!(&copied[..offset], &sentinel[..offset]);
    for y in 0..6 {
        for x in 0..8 {
            let start = offset + y * stride + x * 4;
            assert_eq!(
                &copied[start..start + 3],
                &[100, (y * 31) as u8, (x * 23) as u8]
            );
        }
        assert_eq!(
            &copied[offset + y * stride + 32..offset + (y + 1) * stride],
            &[0xab; 4]
        );
    }
    let region_buffer = pool.create_buffer(
        offset as i32,
        3,
        2,
        16,
        wl_shm::Format::Xrgb8888,
        &client.handle(),
        (),
    );
    let region = manager.capture_output_region(0, &client_output, 1, 2, 3, 2, &client.handle(), 3);
    client.dispatch(&mut display, &mut state);
    region.copy(&region_buffer);
    client.dispatch(&mut display, &mut state);
    let framebuffer = renderer.bind(&mut target).unwrap();
    process_frame_captures(
        &mut state,
        &mut renderer,
        &output,
        &framebuffer,
        Transform::Flipped180,
        CursorMode::HostNamed,
    );
    drop(framebuffer);
    assert_eq!(state.pending_captures.len(), 1);
    file.seek(SeekFrom::Start(0)).unwrap();
    file.read_exact(&mut copied).unwrap();
    for y in 0..2 {
        for x in 0..3 {
            let start = offset + y * 16 + x * 4;
            assert_eq!(
                &copied[start..start + 3],
                &[100, ((y + 2) * 31) as u8, ((x + 1) * 23) as u8]
            );
        }
    }
    process_captures(&mut state, &mut renderer, &output, None);
    assert!(state.pending_captures.is_empty());
    client.dispatch(&mut display, &mut state);
    for label in 0..4 {
        let events: Vec<_> = client
            .events
            .screencopy
            .iter()
            .filter(|(id, _)| *id == label)
            .map(|(_, event)| event)
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, zwlr_screencopy_frame_v1::Event::Ready { .. }))
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event, zwlr_screencopy_frame_v1::Event::Failed)));
        let damage = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    zwlr_screencopy_frame_v1::Event::Damage {
                        x: 0,
                        y: 0,
                        width: 8,
                        height: 6
                    }
                )
            })
            .count();
        assert_eq!(damage, usize::from(label == 1));
    }
    state.config.accessibility.zoom = false;
    let frame = manager.capture_output(1, &client_output, &client.handle(), 4);
    client.dispatch(&mut display, &mut state);
    frame.copy(&buffer);
    client.dispatch(&mut display, &mut state);
    let framebuffer = renderer.bind(&mut target).unwrap();
    process_frame_captures(
        &mut state,
        &mut renderer,
        &output,
        &framebuffer,
        Transform::Flipped180,
        CursorMode::HostNamed,
    );
    drop(framebuffer);
    assert!(state.pending_captures.is_empty());
    file.seek(SeekFrom::Start(0)).unwrap();
    file.read_exact(&mut copied).unwrap();
    let mut elements = crate::render::output_elements(
        &mut renderer,
        &mut state,
        &output,
        None,
        CursorMode::Composited,
    );
    elements.push(
        TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            (0.0, 0.0),
            texture,
            1,
            Transform::Normal,
            None,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .into(),
    );
    let mut expected_texture =
        Offscreen::<GlesTexture>::create_buffer(&mut renderer, Fourcc::Abgr8888, (8, 6).into())
            .unwrap();
    let mut framebuffer = renderer.bind(&mut expected_texture).unwrap();
    OutputDamageTracker::new((8, 6), 1.0, Transform::Normal)
        .render_output(
            &mut renderer,
            &mut framebuffer,
            0,
            &elements,
            Color32F::BLACK,
        )
        .unwrap();
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size((8, 6).into()),
            Fourcc::Xrgb8888,
        )
        .unwrap();
    let expected = renderer.map_texture(&mapping).unwrap();
    for y in 0..6 {
        for x in 0..8 {
            for channel in 0..3 {
                let actual = copied[offset + y * stride + x * 4 + channel];
                let expected = expected[(y * 8 + x) * 4 + channel];
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "cursor pixel ({x},{y}), channel={channel}: {actual} != {expected}"
                );
            }
        }
    }
    client.dispatch(&mut display, &mut state);
    assert!(client
        .events
        .screencopy
        .iter()
        .any(|(label, event)| *label == 4
            && matches!(event, zwlr_screencopy_frame_v1::Event::Ready { .. })));
}
