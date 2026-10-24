//! Keep collapsed content out of GPUI's list entirely. Even a large metadata
//! fallback costs one header to lay out until it is expanded.
use super::*;

#[derive(Default)]
pub(super) struct VisibleBlocks {
    spans: Vec<VisibleSpan>,
    len: usize,
    scanned: usize,
}

struct VisibleSpan {
    blocks: std::ops::Range<usize>,
    first_row: usize,
}

impl VisibleBlocks {
    pub fn reset(&mut self, blocks: &[Arc<Block>], expanded: &HashSet<usize>) {
        self.spans.clear();
        self.len = 0;
        self.scanned = 0;
        self.extend(blocks, expanded);
    }

    pub fn extend(&mut self, blocks: &[Arc<Block>], expanded: &HashSet<usize>) {
        for (ix, block) in blocks.iter().enumerate().skip(self.scanned) {
            if block
                .kind
                .metadata_section()
                .is_none_or(|section| expanded.contains(&section))
            {
                if let Some(last) = self.spans.last_mut()
                    && last.blocks.end == ix
                {
                    last.blocks.end = ix + 1;
                } else {
                    self.spans.push(VisibleSpan {
                        blocks: ix..ix + 1,
                        first_row: self.len,
                    });
                }
                self.len += 1;
            }
        }
        self.scanned = blocks.len();
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn block(&self, row: usize) -> Option<usize> {
        let span = self.spans.get(
            self.spans
                .partition_point(|span| span.first_row <= row)
                .saturating_sub(1),
        )?;
        let block = span.blocks.start + row.saturating_sub(span.first_row);
        (row < self.len && block < span.blocks.end).then_some(block)
    }

    /// Hidden rows resolve to their visible header.
    pub fn row(&self, block: usize) -> usize {
        let Some(span) = self.spans.get(
            self.spans
                .partition_point(|span| span.blocks.start <= block)
                .saturating_sub(1),
        ) else {
            return 0;
        };
        span.first_row
            + block
                .saturating_sub(span.blocks.start)
                .min(span.blocks.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(visible: &VisibleBlocks) -> Vec<usize> {
        (0..visible.len())
            .map(|row| visible.block(row).unwrap())
            .collect()
    }

    #[test]
    fn collapse_expand_append_and_navigation_keep_document_indices() {
        let doc = parse("---\na: one\nb: two\n---\n# Body\n\nText");
        let mut visible = VisibleBlocks::default();
        let mut expanded = HashSet::new();
        visible.extend(&doc.blocks[..2], &expanded);
        assert_eq!(rows(&visible), [0]);
        visible.extend(&doc.blocks, &expanded);
        assert_eq!(rows(&visible), [0, 3, 4]);
        assert_eq!(visible.row(2), 0);
        assert_eq!(visible.row(3), 1);
        assert_eq!(visible.block(2), Some(4));
        expanded.insert(0);
        visible.reset(&doc.blocks, &expanded);
        assert_eq!(rows(&visible), [0, 1, 2, 3, 4]);
        expanded.clear();
        visible.reset(&doc.blocks, &expanded);
        assert_eq!(rows(&visible), [0, 3, 4]);
        assert_eq!(visible.spans.len(), 2);
        assert_eq!(visible.block(visible.len()), None);
    }

    #[test]
    fn huge_fallback_is_one_visible_item_when_collapsed() {
        let doc = parse(&format!(
            "---\nvalue: {}\n---\n# End\n",
            "x".repeat(100_000)
        ));
        let mut visible = VisibleBlocks::default();
        visible.reset(&doc.blocks, &HashSet::new());
        assert_eq!(visible.len(), 2);
        assert_eq!(visible.block(1), Some(doc.blocks.len() - 1));
    }

    #[test]
    fn multiple_sections_and_streaming_use_compact_spans() {
        let blocks: Vec<_> = (0..20)
            .map(|ix| {
                Arc::new(Block::new(
                    if (1..4).contains(&ix) {
                        BlockKind::FrontMatterSource { section: 0 }
                    } else if (9..12).contains(&ix) {
                        BlockKind::FrontMatterSource { section: 8 }
                    } else {
                        BlockKind::Paragraph
                    },
                    ix..ix + 1,
                ))
            })
            .collect();
        for expanded in [
            HashSet::new(),
            HashSet::from([0]),
            HashSet::from([8]),
            HashSet::from([0, 8]),
        ] {
            let expected: Vec<_> = blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.kind
                        .metadata_section()
                        .is_none_or(|section| expanded.contains(&section))
                })
                .map(|(ix, _)| ix)
                .collect();
            let mut visible = VisibleBlocks::default();
            for end in 1..=blocks.len() {
                visible.extend(&blocks[..end], &expanded);
            }
            assert_eq!(rows(&visible), expected);
            for ix in 0..blocks.len() {
                assert_eq!(
                    visible.row(ix),
                    expected
                        .partition_point(|&block| block <= ix)
                        .saturating_sub(1)
                );
            }
            assert_eq!(visible.spans.len(), 3 - expanded.len());
        }
    }
}
