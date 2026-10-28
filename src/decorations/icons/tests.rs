use super::*;

fn bitmap(bytes: usize) -> Rc<RgbaBitmap> {
    Rc::new(RgbaBitmap {
        pixels: vec![0; bytes],
        width: 1,
        height: 1,
    })
}

#[test]
fn resolver_initialization_is_deferred_until_an_icon_is_requested() {
    let mut cache = IconCache::new(Some("hicolor"));
    assert!(cache.resolver.get().is_none());
    cache.reconfigure(Some("Adwaita"));
    assert!(cache.resolver.get().is_none());
    cache.get("definitely-not-an-installed-app-xyz", 24);
    assert!(cache.resolver.get().is_some());
    assert!(!cache.entries.borrow().entries.is_empty());
    cache.reconfigure(None);
    assert!(cache.resolver.get().is_none());
    assert!(cache.entries.borrow().entries.is_empty());
    assert_eq!(cache.entries.borrow().bytes, 0);
}

#[test]
fn borrowed_lookups_preserve_sizes_and_cache_hits_do_not_allocate() {
    let cache = IconCache::default();
    let pixels = bitmap(16);
    cache
        .entries
        .borrow_mut()
        .insert(("test".to_owned(), 24), Some(pixels.clone()));
    cache
        .entries
        .borrow_mut()
        .insert(("test".to_owned(), 48), None);
    assert!(Rc::ptr_eq(&cache.get("test", 24).unwrap(), &pixels));
    assert!(cache.get("test", 48).is_none());
    let allocations = crate::test_support::count_allocations(|| {
        for _ in 0..1000 {
            std::hint::black_box(cache.get("test", 24));
            std::hint::black_box(cache.get("test", 48));
        }
    });
    assert_eq!(allocations, 0);
    assert!(cache.resolver.get().is_none());
    assert!(cache.entries.borrow_mut().get("missing", 24).is_none());
    assert!(cache.entries.borrow_mut().get("test", 96).is_none());
}

#[test]
fn negative_results_obey_the_entry_budget_and_keep_recently_used_icons() {
    let mut entries = IconEntries::default();
    let keep = ("keep".to_owned(), 24);
    let evict = ("evict".to_owned(), 24);
    entries.insert(keep.clone(), None);
    entries.insert(evict.clone(), None);
    for index in 2..MAX_CACHE_ENTRIES {
        entries.insert((format!("missing-{index}"), 24), None);
    }
    assert!(entries.get(&keep.0, keep.1).unwrap().is_none());
    entries.insert(("new".to_owned(), 24), None);
    assert_eq!(entries.entries.len(), MAX_CACHE_ENTRIES);
    assert!(entries.get(&keep.0, keep.1).is_some());
    assert!(entries.get(&evict.0, evict.1).is_none());
    for index in 0..10_000 {
        entries.insert((format!("churn-{index}"), 24), None);
    }
    assert_eq!(entries.entries.len(), MAX_CACHE_ENTRIES);
    assert!(entries.bytes <= MAX_CACHE_BYTES);
}

#[test]
fn pixel_budget_evicts_old_icons_without_invalidating_external_references() {
    let mut entries = IconEntries::default();
    let first = bitmap(MAX_CACHE_BYTES / 2);
    let weak = Rc::downgrade(&first);
    let key = ("first".to_owned(), 24);
    entries.insert(key.clone(), Some(first.clone()));
    entries.insert(("second".to_owned(), 24), Some(bitmap(MAX_CACHE_BYTES / 2)));
    assert!(entries.get(&key.0, key.1).is_none());
    assert_eq!(entries.entries.len(), 1);
    assert!(entries.bytes <= MAX_CACHE_BYTES);
    assert_eq!(first.pixels.len(), MAX_CACHE_BYTES / 2);
    drop(first);
    assert!(weak.upgrade().is_none());
}

#[test]
fn oversized_pixels_and_app_ids_are_not_retained_or_allowed_to_flush_the_cache() {
    let mut entries = IconEntries::default();
    let keep = ("keep".to_owned(), 24);
    entries.insert(keep.clone(), None);
    entries.insert(("large".to_owned(), 24), Some(bitmap(MAX_CACHE_BYTES + 1)));
    entries.insert(("x".repeat(MAX_CACHE_BYTES + 1), 24), None);
    assert_eq!(entries.entries.len(), 1);
    assert!(entries.get(&keep.0, keep.1).is_some());
}

#[test]
fn replacing_an_entry_releases_its_previous_pixels_and_updates_the_budget() {
    let mut entries = IconEntries::default();
    let pixels = bitmap(1024);
    let weak = Rc::downgrade(&pixels);
    let key = ("replace".to_owned(), 24);
    entries.insert(key.clone(), Some(pixels));
    assert_eq!(entries.bytes, 1024 + key.0.len());
    entries.insert(key.clone(), None);
    assert_eq!(entries.bytes, key.0.len());
    assert!(weak.upgrade().is_none());
    assert_eq!(entries.entries.len(), 1);
}

#[test]
fn switching_icon_theme_replaces_cached_pixels() {
    let root = std::env::temp_dir().join(format!("blair-icon-themes-{}", std::process::id()));
    let paths = delib::xdg::XdgPaths {
        data_home: root.join("data"),
        data_dirs: vec![],
        config_home: root.join("config"),
        config_dirs: vec![],
    };
    std::fs::create_dir_all(paths.data_home.join("applications")).unwrap();
    std::fs::write(
        paths.data_home.join("applications/test.desktop"),
        "[Desktop Entry]\nType=Application\nName=Test\nExec=true\nIcon=test-icon\n",
    )
    .unwrap();
    for (theme, pixel) in [("Red", [255, 0, 0, 255]), ("Blue", [0, 0, 255, 255])] {
        let directory = paths.data_home.join("icons").join(theme);
        std::fs::create_dir_all(directory.join("24x24/apps")).unwrap();
        std::fs::write(directory.join("index.theme"), format!("[Icon Theme]\nName={theme}\nDirectories=24x24/apps\n[24x24/apps]\nSize=24\nType=Fixed\nContext=Applications\n")).unwrap();
        image::RgbaImage::from_pixel(24, 24, image::Rgba(pixel))
            .save(directory.join("24x24/apps/test-icon.png"))
            .unwrap();
    }
    let mut cache = IconCache::with_resolver(ApplicationIconResolver::with_paths(&paths, "Red"));
    let red = cache.get("test", 24).unwrap();
    assert_eq!(&red.pixels[..4], &[255, 0, 0, 255]);
    assert!(Rc::ptr_eq(&red, &cache.get("test", 24).unwrap()));
    cache.replace_resolver(ApplicationIconResolver::with_paths(&paths, "Blue"));
    let blue = cache.get("test", 24).unwrap();
    assert_eq!(&blue.pixels[..4], &[0, 0, 255, 255]);
    assert!(!Rc::ptr_eq(&red, &blue));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn decodes_and_resizes_a_real_png() {
    let root = std::env::temp_dir().join(format!("blair-icons-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("icon.png");
    let source = image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 20, 30, 255]));
    source.save(&path).unwrap();

    let bitmap = decode_and_resize(&path, 24).unwrap();
    assert_eq!(bitmap.width, 24);
    assert_eq!(bitmap.height, 24);
    assert_eq!(&bitmap.pixels[0..4], &[200, 20, 30, 255]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_apps_are_cached_as_a_negative_result() {
    let cache = IconCache::default();
    assert!(cache
        .get("definitely-not-an-installed-app-xyz", 24)
        .is_none());
    assert!(cache
        .get("definitely-not-an-installed-app-xyz", 24)
        .is_none());
    assert_eq!(cache.entries.borrow().entries.len(), 1);
}

struct Fixture {
    root: std::path::PathBuf,
    paths: delib::xdg::XdgPaths,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("blair-icon-{label}-{}", std::process::id()));
        let paths = delib::xdg::XdgPaths {
            data_home: root.join("data"),
            data_dirs: vec![],
            config_home: root.join("config"),
            config_dirs: vec![],
        };
        std::fs::create_dir_all(paths.data_home.join("applications")).unwrap();
        std::fs::write(
            paths.data_home.join("applications/test.desktop"),
            "[Desktop Entry]\nType=Application\nName=Test\nExec=true\nIcon=test-icon\n",
        )
        .unwrap();
        Self { root, paths }
    }

    fn theme(&self, name: &str, parent: &str) -> std::path::PathBuf {
        let directory = self.paths.data_home.join("icons").join(name);
        std::fs::create_dir_all(directory.join("scalable/apps")).unwrap();
        std::fs::write(directory.join("index.theme"), format!("[Icon Theme]\nName={name}\nInherits={parent}\nDirectories=scalable/apps\n[scalable/apps]\nSize=24\nType=Scalable\nMinSize=1\nMaxSize=2048\nContext=Applications\n")).unwrap();
        directory.join("scalable/apps")
    }

    fn resolver(&self, theme: &str) -> ApplicationIconResolver {
        ApplicationIconResolver::with_paths(&self.paths, theme)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn pixel(bitmap: &RgbaBitmap, x: i32, y: i32) -> &[u8] {
    let index = (y * bitmap.width + x) as usize * 4;
    &bitmap.pixels[index..index + 4]
}

const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="#c86432" opacity="0.5"/></svg>"##;

#[test]
fn svg_only_themes_are_cached_at_each_physical_size_and_invalidated_by_theme_changes() {
    let f = Fixture::new("svg-cache");
    let directory = f.theme("Vector", "");
    std::fs::write(directory.join("test-icon.svg"), RED_SVG).unwrap();
    let mut cache = IconCache::with_resolver(f.resolver("Vector"));
    let normal = cache.get("test", 24).unwrap();
    let scaled = cache.get("test", 48).unwrap();
    assert_eq!(
        (normal.width, normal.height, normal.pixels.len()),
        (24, 24, 24 * 24 * 4)
    );
    assert_eq!((scaled.width, scaled.height), (48, 48));
    assert!(pixel(&normal, 12, 12)[0] >= 199);
    assert_eq!(&pixel(&normal, 12, 12)[1..], &[100, 50, 128]);
    assert_eq!(
        crate::test_support::count_allocations(|| {
            for _ in 0..1000 {
                std::hint::black_box(cache.get("test", 24));
                std::hint::black_box(cache.get("test", 48));
            }
        }),
        0
    );
    assert!(Rc::ptr_eq(&normal, &cache.get("test", 24).unwrap()));
    let other = f.theme("Green", "");
    std::fs::write(
        other.join("test-icon.svg"),
        RED_SVG.replace("#c86432", "#00ff00").replace("0.5", "1"),
    )
    .unwrap();
    cache.replace_resolver(f.resolver("Green"));
    let green = cache.get("test", 24).unwrap();
    assert_eq!(pixel(&green, 12, 12), &[0, 255, 0, 255]);
    assert!(!Rc::ptr_eq(&normal, &green));
}

#[test]
fn svg_upload_premultiplies_transparency_once_at_the_requested_buffer_scale() {
    use smithay::backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement, Kind, RenderElement, UnderlyingStorage,
        },
        pixman::PixmanRenderer,
    };
    let f = Fixture::new("svg-upload");
    let directory = f.theme("Vector", "");
    std::fs::write(directory.join("test-icon.svg"), RED_SVG).unwrap();
    let cache = IconCache::with_resolver(f.resolver("Vector"));
    let bitmap = cache.get("test", 48).unwrap();
    let mut decoration = crate::render::WindowDecoration::default();
    decoration.update_icon(Some("test"), Some(&bitmap), 24, 2);
    let (buffer, size) = decoration.icon_buffer().unwrap();
    assert_eq!(size, (24, 24).into());
    let mut renderer = PixmanRenderer::new().unwrap();
    let element = MemoryRenderBufferRenderElement::from_buffer(
        &mut renderer,
        (0.0, 0.0),
        buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .unwrap();
    let Some(UnderlyingStorage::Memory(memory)) = element.underlying_storage(&mut renderer) else {
        panic!("expected icon memory storage");
    };
    assert_eq!(memory.size(), (48, 48).into());
    assert_eq!(&memory[..4], &[100, 50, 25, 128]);
}

#[test]
fn svg_rasterization_preserves_aspect_ratio_and_bounds_large_intrinsic_sizes() {
    let f = Fixture::new("svg-aspect");
    let directory = f.theme("Vector", "");
    let path = directory.join("test-icon.svg");
    std::fs::write(&path, r##"<svg xmlns="http://www.w3.org/2000/svg" width="2000000" height="1000000" viewBox="0 0 2 1"><rect width="2" height="1" fill="#00ff00"/></svg>"##).unwrap();
    let bitmap = decode_and_resize(&path, 24).unwrap();
    assert_eq!(bitmap.pixels.len(), 24 * 24 * 4);
    assert_eq!(pixel(&bitmap, 12, 0), &[0; 4]);
    assert_eq!(pixel(&bitmap, 12, 12), &[0, 255, 0, 255]);
    assert_eq!(pixel(&bitmap, 12, 23), &[0; 4]);
}

#[test]
fn svg_images_resolve_relative_to_the_document_and_text_uses_system_fonts() {
    let f = Fixture::new("svg-content");
    let directory = f.theme("Vector", "");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255]))
        .save(directory.join("source.png"))
        .unwrap();
    let path = directory.join("test-icon.svg");
    std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="24" height="24"><image xlink:href="source.png" width="24" height="24"/></svg>"#).unwrap();
    assert_eq!(
        pixel(&decode_and_resize(&path, 24).unwrap(), 12, 12),
        &[0, 0, 255, 255]
    );
    std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><text x="0" y="20" font-family="sans-serif" font-size="20">Hi</text></svg>"#).unwrap();
    let bitmap = decode_and_resize(&path, 24).unwrap();
    assert!(bitmap.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
}

#[test]
fn raster_formats_keep_theme_priority_when_parent_themes_have_svg_icons() {
    let f = Fixture::new("svg-raster-priority");
    let child = f.theme("Child", "Parent");
    let parent = f.theme("Parent", "");
    std::fs::write(parent.join("test-icon.svg"), RED_SVG).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([0, 200, 0]))
        .save(child.join("test-icon.jpg"))
        .unwrap();
    let cache = IconCache::with_resolver(f.resolver("Child"));
    let bitmap = cache.get("test", 24).unwrap();
    assert!(pixel(&bitmap, 12, 12)[1] >= 198);
    assert_eq!(pixel(&bitmap, 12, 12)[3], 255);
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]))
        .save(child.join("test-icon.png"))
        .unwrap();
    let cache = IconCache::with_resolver(f.resolver("Child"));
    assert_eq!(
        pixel(&cache.get("test", 24).unwrap(), 12, 12),
        &[0, 0, 255, 255]
    );
    std::fs::write(child.join("test-icon.svg"), RED_SVG).unwrap();
    let cache = IconCache::with_resolver(f.resolver("Child"));
    assert_eq!(pixel(&cache.get("test", 24).unwrap(), 12, 12)[3], 128);
}

#[test]
fn malformed_icons_are_negatively_cached_and_excessive_decode_sizes_are_rejected() {
    let f = Fixture::new("svg-invalid");
    let directory = f.theme("Vector", "");
    let path = directory.join("test-icon.svg");
    std::fs::write(&path, b"not SVG").unwrap();
    let mut cache = IconCache::with_resolver(f.resolver("Vector"));
    assert!(cache.get("test", 24).is_none());
    std::fs::write(&path, RED_SVG).unwrap();
    assert!(cache.get("test", 24).is_none());
    cache.replace_resolver(f.resolver("Vector"));
    assert!(cache.get("test", 24).is_some());
    let empty = IconCache::default();
    assert!(empty.get("test", i32::MAX).is_none());
    assert!(empty.resolver.get().is_none());
    assert!(decode_and_resize(&path, i32::MAX).is_err());
    let huge = directory.join("huge.svg");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    assert!(decode_and_resize(&huge, 24).is_err());
    let png = directory.join("wide.png");
    image::RgbaImage::from_pixel(8193, 1, image::Rgba([0; 4]))
        .save(&png)
        .unwrap();
    assert!(decode_and_resize(&png, 24).is_err());
}
