use std::{
    collections::HashMap,
    rc::{Rc, Weak},
    time::Instant,
};

use smithay::{
    backend::renderer::{
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            Element, Id, Kind, RenderElement, UnderlyingStorage,
        },
        gles::{GlesError, GlesFrame, GlesRenderer},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform},
};

#[cfg(test)]
use smithay::utils::{Logical, Size};

use super::{buffer_from_pixels, rasterize_button, ButtonGlyph, ButtonKey};

const MAX_ENTRIES: usize = 256;

#[derive(Debug, Default)]
pub struct ButtonBuffers {
    entries: HashMap<ButtonKey, CachedButton>,
}

#[derive(Debug)]
struct CachedButton {
    buffer: Weak<MemoryRenderBuffer>,
    last_used: Instant,
}

impl ButtonKey {
    fn normalized(mut self) -> Self {
        if self.look.disc[3] == 0 {
            self.look.disc = [0; 4];
        }
        if !self.look.glyph_visible || self.look.glyph_color[3] == 0 {
            self.look.glyph_visible = false;
            self.look.glyph = ButtonGlyph::Close;
            self.look.glyph_color = [0; 4];
        }
        self
    }
}

impl ButtonBuffers {
    fn get(&mut self, key: ButtonKey) -> Option<Rc<MemoryRenderBuffer>> {
        let key = key.normalized();
        if key.size <= 0 || key.buffer_scale <= 0 {
            return None;
        }
        if let Some(entry) = self.entries.get_mut(&key) {
            if let Some(buffer) = entry.buffer.upgrade() {
                entry.last_used = Instant::now();
                return Some(buffer);
            }
        }
        let Some(size) = key.size.checked_mul(key.buffer_scale) else {
            tracing::warn!(
                size = key.size,
                scale = key.buffer_scale,
                "invalid button buffer dimensions"
            );
            return None;
        };
        let buffer = Rc::new(buffer_from_pixels(
            &rasterize_button(key.look, size),
            size,
            size,
            key.buffer_scale,
        ));
        self.entries
            .retain(|_, entry| entry.buffer.strong_count() > 0);
        if self.entries.len() >= MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
                .unwrap();
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            CachedButton {
                buffer: Rc::downgrade(&buffer),
                last_used: Instant::now(),
            },
        );
        Some(buffer)
    }
}

#[derive(Debug)]
pub(super) struct ButtonCache {
    key: Option<ButtonKey>,
    buffer: Option<Rc<MemoryRenderBuffer>>,
    id: Id,
    commit: CommitCounter,
}

impl Default for ButtonCache {
    fn default() -> Self {
        Self {
            key: None,
            buffer: None,
            id: Id::new(),
            commit: CommitCounter::default(),
        }
    }
}

impl ButtonCache {
    pub(super) fn update(&mut self, key: ButtonKey, buffers: &mut ButtonBuffers) {
        let key = key.normalized();
        if self.key == Some(key) {
            return;
        }
        self.buffer = buffers.get(key);
        self.key = Some(key);
        self.commit.increment();
    }

    pub(super) fn clear(&mut self) {
        if self.key.take().is_some() {
            self.buffer = None;
            self.commit.increment();
        }
    }

    #[cfg(test)]
    pub(super) fn buffer(&self) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>)> {
        self.buffer.as_ref().map(|buffer| {
            (
                buffer.as_ref(),
                (self.key.unwrap().size, self.key.unwrap().size).into(),
            )
        })
    }

    pub(super) fn element(
        &self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Physical>,
        alpha: f32,
    ) -> Result<Option<ButtonRenderElement>, GlesError> {
        self.buffer
            .as_ref()
            .map(|buffer| {
                let size = self.key.unwrap().size;
                let element = MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    location,
                    buffer,
                    Some(alpha),
                    None,
                    Some((size, size).into()),
                    Kind::Unspecified,
                )?;
                Ok(ButtonRenderElement {
                    element,
                    id: self.id.clone(),
                    commit: self.commit,
                })
            })
            .transpose()
    }
}

pub struct ButtonRenderElement {
    element: MemoryRenderBufferRenderElement<GlesRenderer>,
    id: Id,
    commit: CommitCounter,
}

impl Element for ButtonRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.element.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.element.transform()
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
        }
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.element.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.element.alpha()
    }
    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl RenderElement<GlesRenderer> for ButtonRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        self.element.draw(frame, src, dst, damage, opaque_regions)
    }
    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.element.underlying_storage(renderer)
    }
}

#[cfg(test)]
mod tests;
