use gpui::{App, BorrowAppContext, EntityId, Global, RenderImage};
use plastext_platform::images::Region;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const BUDGET: usize = 64 * 1024 * 1024;
struct Entry {
    image: Arc<RenderImage>,
    used: u64,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<(EntityId, Region), Entry>,
    active: HashSet<EntityId>,
    clock: u64,
    bytes: usize,
}
impl Global for Cache {}
impl Cache {
    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
    }
    pub fn active(owner: EntityId, active: bool, cx: &mut App) {
        Self::init(cx);
        cx.update_global::<Self, _>(|cache, cx| {
            let changed = if active {
                cache.active.insert(owner)
            } else {
                cache.active.remove(&owner)
            };
            if changed {
                cx.refresh_windows();
            }
        });
        if !active {
            Self::retain(owner, &[], cx);
        }
    }
    /// Each pane can hold its current frame and a complete replacement.
    pub fn frame_budget(cx: &App) -> usize {
        BUDGET / cx.global::<Self>().active.len().max(1) / 2
    }
    pub fn get(owner: EntityId, region: Region, cx: &mut App) -> Option<Arc<RenderImage>> {
        cx.update_global::<Self, _>(|cache, _| {
            cache.clock += 1;
            let entry = cache.entries.get_mut(&(owner, region))?;
            entry.used = cache.clock;
            Some(entry.image.clone())
        })
    }
    pub fn contains(owner: EntityId, region: Region, cx: &App) -> bool {
        cx.global::<Self>().entries.contains_key(&(owner, region))
    }
    pub fn insert(owner: EntityId, region: Region, image: Arc<RenderImage>, cx: &mut App) {
        cx.update_global::<Self, _>(|cache, cx| {
            if let Some(old) = cache.entries.remove(&(owner, region)) {
                cache.release(old, cx);
            }
            let charged = image.as_bytes(0).unwrap().len() * 2;
            while cache.bytes + charged > BUDGET {
                let Some(key) = cache
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| e.used)
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                let old = cache.entries.remove(&key).unwrap();
                cache.release(old, cx);
            }
            cache.clock += 1;
            cache.bytes += charged;
            cache.entries.insert(
                (owner, region),
                Entry {
                    image,
                    used: cache.clock,
                },
            );
        });
    }
    pub fn retain(owner: EntityId, regions: &[Region], cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        cx.update_global::<Self, _>(|cache, cx| {
            let keys: Vec<_> = cache
                .entries
                .keys()
                .copied()
                .filter(|(id, r)| *id == owner && !regions.contains(r))
                .collect();
            for key in keys {
                let entry = cache.entries.remove(&key).unwrap();
                cache.release(entry, cx);
            }
        });
    }
    fn release(&mut self, entry: Entry, cx: &mut App) {
        self.bytes -= entry.image.as_bytes(0).unwrap().len() * 2;
        cx.defer(move |cx| {
            cx.drop_image(entry.image, None);
        });
    }
}
