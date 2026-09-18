use std::{
    cell::RefCell,
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::OnceLock,
};

use image::imageops::FilterType;

pub struct RgbaBitmap {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

/// Resolves and decodes a window's real app icon by its `app_id`, through
/// its desktop entry and the freedesktop icon theme directories. Misses are
/// cached too, so an app with no icon is only looked up once per size.
#[derive(Default)]
pub struct IconCache {
    entries: RefCell<HashMap<(String, i32), Option<Rc<RgbaBitmap>>>>,
}

impl IconCache {
    pub fn get(&self, app_id: &str, size: i32) -> Option<Rc<RgbaBitmap>> {
        let key = (app_id.to_owned(), size);
        if let Some(cached) = self.entries.borrow().get(&key) {
            return cached.clone();
        }
        let resolved = load_icon(app_id, size);
        self.entries.borrow_mut().insert(key, resolved.clone());
        resolved
    }
}

fn load_icon(app_id: &str, size: i32) -> Option<Rc<RgbaBitmap>> {
    let name = desktop_icon_name(app_id)?;
    let path = resolve_icon_path(&name)?;
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

fn desktop_icon_name(app_id: &str) -> Option<String> {
    let file_name = if app_id.ends_with(".desktop") {
        app_id.to_owned()
    } else {
        format!("{app_id}.desktop")
    };
    xdg_data_directories()
        .into_iter()
        .map(|directory| directory.join("applications").join(&file_name))
        .find(|path| path.is_file())
        .and_then(|path| desktop_entry_icon(&path))
}

fn desktop_entry_icon(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some(value) = line.strip_prefix("Icon=") {
                return (!value.is_empty()).then(|| value.to_owned());
            }
        }
    }
    None
}

fn resolve_icon_path(icon: &str) -> Option<PathBuf> {
    let path = PathBuf::from(icon);
    if path.is_absolute() && is_raster_icon(&path) && path.is_file() {
        return Some(path);
    }
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(icon);
    icon_index().get(&name.to_ascii_lowercase()).cloned()
}

fn icon_index() -> &'static HashMap<String, PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut icons = HashMap::new();
        for directory in xdg_data_directories() {
            collect_icons(&directory.join("icons"), &mut icons);
            collect_icons(&directory.join("pixmaps"), &mut icons);
        }
        icons
    })
}

fn collect_icons(directory: &Path, icons: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_icons(&path, icons);
        } else if is_raster_icon(&path) {
            if let Some(name) = path.file_stem().and_then(|name| name.to_str()) {
                icons.entry(name.to_ascii_lowercase()).or_insert(path);
            }
        }
    }
}

fn is_raster_icon(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg"
            )
        })
}

fn xdg_data_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = env::var_os("XDG_DATA_HOME") {
        directories.push(PathBuf::from(home));
    } else if let Some(home) = env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share"));
    }
    let data_dirs =
        env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    directories.extend(env::split_paths(&data_dirs));
    directories
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("blair-icons-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn reads_icon_only_from_the_desktop_entry_group() {
        let root = temp_dir("desktop-entry");
        let path = root.join("app.desktop");
        fs::write(
            &path,
            "Icon=wrong\n[Desktop Entry]\nName=Example\nIcon=example-icon\n",
        )
        .unwrap();
        assert_eq!(desktop_entry_icon(&path), Some("example-icon".to_owned()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_icon_value_is_treated_as_missing() {
        let root = temp_dir("empty-icon");
        let path = root.join("app.desktop");
        fs::write(&path, "[Desktop Entry]\nIcon=\n").unwrap();
        assert_eq!(desktop_entry_icon(&path), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_raster_extensions_are_indexed() {
        let root = temp_dir("collect");
        let nested = root.join("hicolor/48x48/apps");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("sample.png"), b"not a real image").unwrap();
        fs::write(nested.join("sample.svg"), b"<svg/>").unwrap();

        let mut icons = HashMap::new();
        collect_icons(&root, &mut icons);
        assert_eq!(icons.len(), 1);
        assert!(icons.contains_key("sample"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decodes_and_resizes_a_real_png() {
        let root = temp_dir("decode");
        let path = root.join("icon.png");
        let source = image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 20, 30, 255]));
        source.save(&path).unwrap();

        let bitmap = decode_and_resize(&path, 24).unwrap();
        assert_eq!(bitmap.width, 24);
        assert_eq!(bitmap.height, 24);
        assert_eq!(&bitmap.pixels[0..4], &[200, 20, 30, 255]);
        fs::remove_dir_all(root).unwrap();
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
