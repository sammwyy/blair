use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc};

use delib::{application_icons::ApplicationIconResolver, icons::system_icon_theme};
use image::imageops::FilterType;

pub struct RgbaBitmap {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

/// Resolves and decodes a window's real app icon by its `app_id`, through
/// its desktop entry and the freedesktop icon theme directories. Misses are
/// cached too, so an app with no icon is only looked up once per size.
pub struct IconCache {
    entries: RefCell<HashMap<(String, i32), Option<Rc<RgbaBitmap>>>>,
    resolver: ApplicationIconResolver,
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new(None)
    }
}

impl IconCache {
    pub fn new(theme: Option<&str>) -> Self {
        Self::with_resolver(ApplicationIconResolver::new(
            theme.map(str::to_owned).unwrap_or_else(system_icon_theme),
        ))
    }

    fn with_resolver(resolver: ApplicationIconResolver) -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
            resolver,
        }
    }

    pub fn reconfigure(&mut self, theme: Option<&str>) {
        self.replace_resolver(ApplicationIconResolver::new(
            theme.map(str::to_owned).unwrap_or_else(system_icon_theme),
        ));
    }

    fn replace_resolver(&mut self, resolver: ApplicationIconResolver) {
        self.resolver = resolver;
        self.entries.get_mut().clear();
    }
}

impl IconCache {
    pub fn get(&self, app_id: &str, size: i32) -> Option<Rc<RgbaBitmap>> {
        let key = (app_id.to_owned(), size);
        if let Some(cached) = self.entries.borrow().get(&key) {
            return cached.clone();
        }
        let resolved = load_icon(&self.resolver, app_id, size);
        self.entries.borrow_mut().insert(key, resolved.clone());
        resolved
    }
}

fn load_icon(
    resolver: &ApplicationIconResolver,
    app_id: &str,
    size: i32,
) -> Option<Rc<RgbaBitmap>> {
    let path = resolver.resolve_raster(app_id, size.max(1) as u32, 1)?;
    Some(Rc::new(decode_and_resize(&path, size)?))
}

fn decode_and_resize(path: &Path, size: i32) -> Option<RgbaBitmap> {
    let size = size.max(1) as u32;
    let decoded = image::open(path).ok()?.into_rgba8();
    let resized = if decoded.width() == size && decoded.height() == size {
        decoded
    } else {
        image::imageops::resize(&decoded, size, size, FilterType::Triangle)
    };
    Some(RgbaBitmap {
        width: resized.width() as i32,
        height: resized.height() as i32,
        pixels: resized.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut cache =
            IconCache::with_resolver(ApplicationIconResolver::with_paths(&paths, "Red"));
        let red = cache.get("test", 24).unwrap();
        assert_eq!(&red.pixels[..4], &[255, 0, 0, 255]);
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
    }
}
