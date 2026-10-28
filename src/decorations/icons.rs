use std::{
    cell::{OnceCell, RefCell},
    collections::hash_map::RandomState,
    hash::{Hash, Hasher},
    rc::Rc,
    time::Instant,
};

use delib::{application_icons::ApplicationIconResolver, icons::system_icon_theme};
use hashbrown::{Equivalent, HashMap};
mod decode;
use decode::{decode_and_resize, supported_size};

const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

type IconKey = (String, i32);

pub struct RgbaBitmap {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

struct IconLookup<'a>(&'a str, i32);

impl Hash for IconLookup<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0, self.1).hash(state);
    }
}

impl Equivalent<IconKey> for IconLookup<'_> {
    fn equivalent(&self, key: &IconKey) -> bool {
        self.0 == key.0 && self.1 == key.1
    }
}

struct CachedIcon {
    bitmap: Option<Rc<RgbaBitmap>>,
    last_used: Instant,
    bytes: usize,
}

#[derive(Default)]
struct IconEntries {
    entries: HashMap<IconKey, CachedIcon, RandomState>,
    bytes: usize,
}

impl IconEntries {
    fn get(&mut self, app_id: &str, size: i32) -> Option<Option<Rc<RgbaBitmap>>> {
        let entry = self.entries.get_mut(&IconLookup(app_id, size))?;
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
        let size = size.max(1);
        if !supported_size(size) {
            return None;
        }
        if let Some(cached) = self.entries.borrow_mut().get(app_id, size) {
            return cached;
        }
        let resolver = self.resolver.get_or_init(|| {
            ApplicationIconResolver::new(self.theme.clone().unwrap_or_else(system_icon_theme))
        });
        let resolved = load_icon(resolver, app_id, size);
        self.entries
            .borrow_mut()
            .insert((app_id.to_owned(), size), resolved.clone());
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
    let path = resolver.resolve_with_extensions(
        app_id,
        size.max(1) as u32,
        1,
        &["svg", "png", "jpg", "jpeg"],
    )?;
    match decode_and_resize(&path, size) {
        Ok(bitmap) => Some(Rc::new(bitmap)),
        Err(error) => {
            tracing::debug!(%app_id, path = %path.display(), %error, "could not decode application icon");
            None
        }
    }
}

#[cfg(test)]
mod tests;
