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
        Self {
            entries: RefCell::new(HashMap::new()),
            resolver: ApplicationIconResolver::new(system_icon_theme()),
        }
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
