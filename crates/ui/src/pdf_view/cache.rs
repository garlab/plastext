use gpui::{AnyWindowHandle, App, BorrowAppContext, EntityId, Global, RenderImage};
use plastext_platform::pdf::Tile;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const BUDGET: usize = 128 * 1024 * 1024;
type Key = (EntityId, Tile);
struct Entry {
    image: Arc<RenderImage>,
    window: AnyWindowHandle,
    charged: usize,
    used: u64,
    pinned: bool,
    prefetched: bool,
}
#[derive(Default)]
pub(super) struct Tiles {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    clock: u64,
    active: HashSet<EntityId>,
    displayed: HashMap<EntityId, Vec<Tile>>,
}
impl Global for Tiles {}

impl Tiles {
    pub fn set_active(owner: EntityId, active: bool, cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
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
            Self::remove(owner, cx);
        }
    }
    pub fn tile_budget(cx: &App) -> usize {
        // Leave equal room for the displayed viewport and its replacement.
        BUDGET / cx.global::<Self>().active.len().max(1) / (512 * 512 * 8) / 2
    }
    pub fn prefetch_tile_budget(cx: &App) -> usize {
        // Four neighbors together use at most a quarter of this view's share.
        Self::tile_budget(cx) / 8
    }
    pub fn contains(owner: EntityId, tile: Tile, cx: &App) -> bool {
        cx.global::<Self>().entries.contains_key(&(owner, tile))
    }
    pub fn pin(owner: EntityId, tiles: &[Tile], cx: &mut App) {
        cx.update_global::<Self, _>(|cache, _| {
            if cache.displayed.get(&owner).is_some_and(|old| old == tiles) {
                return;
            }
            for ((id, tile), entry) in &mut cache.entries {
                if *id == owner {
                    entry.pinned = tiles.contains(tile);
                    if entry.pinned {
                        entry.prefetched = false;
                    }
                }
            }
            cache.displayed.insert(owner, tiles.to_vec());
        });
    }
    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
    }
    pub fn get(owner: EntityId, tile: Tile, cx: &mut App) -> Option<Arc<RenderImage>> {
        cx.update_global::<Self, _>(|cache, _| {
            cache.clock += 1;
            let entry = cache.entries.get_mut(&(owner, tile))?;
            entry.used = cache.clock;
            Some(entry.image.clone())
        })
    }
    pub fn insert(
        owner: EntityId,
        tile: Tile,
        image: Arc<RenderImage>,
        window: AnyWindowHandle,
        cx: &mut App,
    ) {
        // Charge both retained CPU pixels and a GPU copy. Atlas allocation
        // granularity/driver overhead is measured separately, not hidden by this.
        let charged = image.as_bytes(0).unwrap().len() * 2;
        cx.update_global::<Self, _>(|cache, cx| {
            let mut evicted = Vec::new();
            if let Some(old) = cache.entries.remove(&(owner, tile)) {
                cache.bytes -= old.charged;
                evicted.push(old);
            }
            let share = BUDGET / cache.active.len().max(1);
            let mut owner_bytes: usize = cache
                .entries
                .iter()
                .filter(|(key, _)| key.0 == owner)
                .map(|(_, entry)| entry.charged)
                .sum();
            while owner_bytes + charged > share {
                let Some(key) = cache
                    .entries
                    .iter()
                    .filter(|(key, _)| key.0 == owner)
                    .min_by_key(|(_, entry)| (entry.pinned, !entry.prefetched, entry.used))
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                let entry = cache.entries.remove(&key).unwrap();
                owner_bytes -= entry.charged;
                cache.bytes -= entry.charged;
                evicted.push(entry);
            }
            while cache.bytes + charged > BUDGET {
                let Some(key) = cache
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| (entry.pinned, !entry.prefetched, entry.used))
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                let entry = cache.entries.remove(&key).unwrap();
                cache.bytes -= entry.charged;
                evicted.push(entry);
            }
            cache.clock += 1;
            cache.bytes += charged;
            cache.entries.insert(
                (owner, tile),
                Entry {
                    image,
                    window,
                    charged,
                    used: cache.clock,
                    prefetched: false,
                    pinned: cache
                        .displayed
                        .get(&owner)
                        .is_some_and(|tiles| tiles.contains(&tile)),
                },
            );
            release(evicted, cx);
        });
    }
    pub fn insert_prefetched(
        owner: EntityId,
        tile: Tile,
        image: Arc<RenderImage>,
        window: AnyWindowHandle,
        cx: &mut App,
    ) -> bool {
        let charged = image.as_bytes(0).unwrap().len() * 2;
        cx.update_global::<Self, _>(|cache, cx| {
            if cache.entries.contains_key(&(owner, tile)) {
                return true;
            }
            let share = BUDGET / cache.active.len().max(1);
            let mut owner_bytes = 0;
            let mut prefetched_bytes = 0;
            let mut disposable_bytes = 0;
            for ((id, _), entry) in &cache.entries {
                if *id == owner {
                    owner_bytes += entry.charged;
                    if entry.prefetched {
                        prefetched_bytes += entry.charged;
                    } else if !entry.pinned {
                        disposable_bytes += entry.charged;
                    }
                }
            }
            // Speculation only replaces this view's old, unpinned foreground
            // tiles. Never evict another pane's displayed or in-progress frame,
            // or cycle endlessly through the other preloaded neighbors.
            if prefetched_bytes + charged > share / 4
                || owner_bytes + charged > share + disposable_bytes
                || cache.bytes + charged > BUDGET + disposable_bytes
            {
                return false;
            }
            let mut evicted = Vec::new();
            while owner_bytes + charged > share || cache.bytes + charged > BUDGET {
                let key = cache
                    .entries
                    .iter()
                    .filter(|((id, _), entry)| *id == owner && !entry.pinned && !entry.prefetched)
                    .min_by_key(|(_, entry)| entry.used)
                    .map(|(key, _)| *key)
                    .expect("checked disposable tile capacity");
                let entry = cache.entries.remove(&key).unwrap();
                owner_bytes -= entry.charged;
                cache.bytes -= entry.charged;
                evicted.push(entry);
            }
            cache.clock += 1;
            cache.bytes += charged;
            cache.entries.insert(
                (owner, tile),
                Entry {
                    image,
                    window,
                    charged,
                    used: cache.clock,
                    pinned: false,
                    prefetched: true,
                },
            );
            release(evicted, cx);
            true
        })
    }
    pub fn retain_prefetched(owner: EntityId, pages: &[u32], cx: &mut App) {
        cx.update_global::<Self, _>(|cache, cx| {
            let keys: Vec<_> = cache
                .entries
                .iter()
                .filter(|((id, tile), entry)| {
                    *id == owner && entry.prefetched && !pages.contains(&tile.page)
                })
                .map(|(key, _)| *key)
                .collect();
            let evicted = keys
                .into_iter()
                .map(|key| {
                    let entry = cache.entries.remove(&key).unwrap();
                    cache.bytes -= entry.charged;
                    entry
                })
                .collect();
            release(evicted, cx);
        });
    }
    pub fn remove(owner: EntityId, cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        cx.update_global::<Self, _>(|cache, cx| {
            cache.displayed.remove(&owner);
            let keys: Vec<_> = cache
                .entries
                .keys()
                .filter(|key| key.0 == owner)
                .copied()
                .collect();
            let evicted = keys
                .into_iter()
                .map(|key| {
                    let entry = cache.entries.remove(&key).unwrap();
                    cache.bytes -= entry.charged;
                    entry
                })
                .collect();
            release(evicted, cx);
        });
    }
}
fn release(entries: Vec<Entry>, cx: &mut App) {
    // Dropping the Rust image alone does not evict GPUI's GPU atlas entry.
    cx.defer(move |cx| {
        for entry in entries {
            let _ = entry
                .window
                .update(cx, |_, window, _| window.drop_image(entry.image));
        }
    });
}
