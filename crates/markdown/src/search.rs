use crate::Block;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedMatch {
    pub block: usize,
    pub text: Range<usize>,
    pub source: usize,
}

/// Case-sensitive literal matching of rendered text, including text split by
/// inline markup. Call with bounded block batches on a worker thread.
pub fn find_rendered(
    blocks: &[Arc<Block>],
    first: usize,
    query: &str,
    limit: usize,
) -> Vec<RenderedMatch> {
    let mut hits = Vec::new();
    if query.is_empty() || limit == 0 {
        return hits;
    }
    for (i, block) in blocks.iter().enumerate() {
        for (at, _) in block.text.match_indices(query) {
            hits.push(RenderedMatch {
                block: first + i,
                text: at..at + query.len(),
                source: block.source_at(at),
            });
            if hits.len() == limit {
                return hits;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_across_inline_markup_and_maps_entities_to_source() {
        let text = "**Hello** *world* &amp; café\n\nworld";
        let doc = crate::parse(text);
        let hits = find_rendered(&doc.blocks, 0, "Hello world", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, text.find("Hello").unwrap());
        assert_eq!(
            find_rendered(&doc.blocks, 0, "&", 10)[0].source,
            text.find("&amp;").unwrap()
        );
        assert_eq!(find_rendered(&doc.blocks, 0, "world", 1).len(), 1);
        assert!(find_rendered(&doc.blocks, 0, "", 10).is_empty());
        assert_eq!(find_rendered(&doc.blocks[1..], 1, "world", 10)[0].block, 1);
    }
}
