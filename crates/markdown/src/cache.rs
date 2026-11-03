use crate::{Block, BlockKind};
use std::{collections::HashMap, sync::Arc};

pub const PARSED_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Conservative charge: shared strings/alignments can be counted more than once.
pub fn parsed_bytes(blocks: &[Arc<Block>]) -> usize {
    std::mem::size_of_val(blocks)
        + blocks
            .iter()
            .map(|block| {
                size_of::<Block>()
                    + 2 * size_of::<usize>()
                    + block.text.capacity()
                    + block.spans.capacity() * size_of::<crate::Span>()
                    + block
                        .spans
                        .iter()
                        .filter_map(|s| s.style.link.as_ref())
                        .map(|s| s.len() + 32)
                        .sum::<usize>()
                    + block.marker.as_ref().map_or(0, String::capacity)
                    + block.quote.as_ref().map_or(0, String::capacity)
                    + match &block.kind {
                        BlockKind::TableRow {
                            cells, alignments, ..
                        } => {
                            cells.capacity() * size_of::<std::ops::Range<usize>>()
                                + alignments.len()
                        }
                        BlockKind::Heading { id, .. } => id.capacity(),
                        BlockKind::Code { language, .. } => language.capacity(),
                        BlockKind::Image {
                            destination,
                            title,
                            attributes,
                        } => {
                            destination.capacity()
                                + title.capacity()
                                + attributes.as_ref().map_or(0, |a| a.retained_bytes())
                        }
                        BlockKind::FrontMatter { error, .. } => {
                            error.as_ref().map_or(0, String::capacity)
                        }
                        _ => 0,
                    }
            })
            .sum::<usize>()
}

struct Cached {
    blocks: Vec<Arc<Block>>,
    bytes: usize,
    used: u64,
}
pub struct PageCache {
    pages: HashMap<usize, Cached>,
    bytes: usize,
    budget: usize,
    clock: u64,
}
impl Default for PageCache {
    fn default() -> Self {
        Self::new(PARSED_CACHE_BYTES)
    }
}
impl PageCache {
    pub fn new(budget: usize) -> Self {
        Self {
            pages: HashMap::new(),
            bytes: 0,
            budget,
            clock: 0,
        }
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
    pub fn contains(&self, page: usize) -> bool {
        self.pages.contains_key(&page)
    }
    pub fn clear(&mut self) {
        self.pages.clear();
        self.bytes = 0;
    }
    pub fn get(&mut self, page: usize, local: usize) -> Option<Arc<Block>> {
        self.clock += 1;
        let cached = self.pages.get_mut(&page)?;
        cached.used = self.clock;
        cached.blocks.get(local).cloned()
    }
    pub fn insert(&mut self, page: usize, blocks: Vec<Arc<Block>>) -> bool {
        let bytes = parsed_bytes(&blocks);
        if bytes > self.budget {
            return false;
        }
        if let Some(old) = self.pages.remove(&page) {
            self.bytes -= old.bytes;
        }
        while self.bytes + bytes > self.budget {
            let key = *self
                .pages
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .unwrap()
                .0;
            self.bytes -= self.pages.remove(&key).unwrap().bytes;
        }
        self.clock += 1;
        self.bytes += bytes;
        self.pages.insert(
            page,
            Cached {
                blocks,
                bytes,
                used: self.clock,
            },
        );
        true
    }
}
