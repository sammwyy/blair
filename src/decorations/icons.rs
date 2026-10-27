use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
    path::Path,
    rc::Rc,
    time::Instant,
};

use delib::{application_icons::ApplicationIconResolver, icons::system_icon_theme};
use image::imageops::FilterType;

const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

type IconKey = (String, i32);

pub struct RgbaBitmap {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

struct CachedIcon {
    bitmap: Option<Rc<RgbaBitmap>>,
    last_used: Instant,
    bytes: usize,
}

#[derive(Default)]
struct IconEntries {
    entries: HashMap<IconKey, CachedIcon>,
    bytes: usize,
}

impl IconEntries {
    fn get(&mut self, key: &IconKey) -> Option<Option<Rc<RgbaBitmap>>> {
        let entry = self.entries.get_mut(key)?;
        entry.last_used = Instant::now();
        Some(entry.bitmap.clone())
    }

    fn insert(&mut self, key: IconKey, bitmap: Option<Rc<RgbaBitmap>>) {
        let bytes = key
            .0
            .capacity()
            .saturating_add(bitmap.as_ref().map_or(0, |bitmap| bitmap.pixels.capacity()));
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes -= previous.bytes;
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES || self.bytes > MAX_CACHE_BYTES - bytes {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
                .expect("cache budget exceeded with no entries");
            self.bytes -= self.entries.remove(&oldest).unwrap().bytes;
        }
        self.entries.insert(
            key,
            CachedIcon {
                bitmap,
                last_used: Instant::now(),
                bytes,
            },
        );
        self.bytes += bytes;
    }
}

/// Resolves application icons on demand and retains recently used pixels and
/// misses within fixed byte and entry budgets.
pub struct IconCache {
    entries: RefCell<IconEntries>,
    resolver: OnceCell<ApplicationIconResolver>,
    theme: Option<String>,
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new(None)
    }
}

impl IconCache {
    pub fn new(theme: Option<&str>) -> Self {
        Self {
            entries: RefCell::default(),
            resolver: OnceCell::new(),
            theme: theme.map(str::to_owned),
        }
    }

    pub fn reconfigure(&mut self, theme: Option<&str>) {
        *self = Self::new(theme);
    }

    pub fn get(&self, app_id: &str, size: i32) -> Option<Rc<RgbaBitmap>> {
        let key = (app_id.to_owned(), size.max(1));
        if let Some(cached) = self.entries.borrow_mut().get(&key) {
            return cached;
        }
        let resolver = self.resolver.get_or_init(|| {
            ApplicationIconResolver::new(self.theme.clone().unwrap_or_else(system_icon_theme))
        });
        let resolved = load_icon(resolver, app_id, key.1);
        self.entries.borrow_mut().insert(key, resolved.clone());
        resolved
    }

    #[cfg(test)]
    fn with_resolver(resolver: ApplicationIconResolver) -> Self {
        Self {
            resolver: OnceCell::from(resolver),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn replace_resolver(&mut self, resolver: ApplicationIconResolver) {
        self.reconfigure(None);
        self.resolver = OnceCell::from(resolver);
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
    fn negative_results_obey_the_entry_budget_and_keep_recently_used_icons() {
        let mut entries = IconEntries::default();
        let keep = ("keep".to_owned(), 24);
        let evict = ("evict".to_owned(), 24);
        entries.insert(keep.clone(), None);
        entries.insert(evict.clone(), None);
        for index in 2..MAX_CACHE_ENTRIES {
            entries.insert((format!("missing-{index}"), 24), None);
        }
        assert!(entries.get(&keep).unwrap().is_none());
        entries.insert(("new".to_owned(), 24), None);
        assert_eq!(entries.entries.len(), MAX_CACHE_ENTRIES);
        assert!(entries.get(&keep).is_some());
        assert!(entries.get(&evict).is_none());
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
        assert!(entries.get(&key).is_none());
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
        assert!(entries.get(&keep).is_some());
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
        let mut cache =
            IconCache::with_resolver(ApplicationIconResolver::with_paths(&paths, "Red"));
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
}
