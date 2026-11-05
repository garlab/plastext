use super::*;
use plastext_buffer::{ByteSource as _, FileSource};
use std::sync::Arc;

fn indexed(text: &str) -> (DocumentIndex, Vec<Arc<Block>>) {
    let source = FileSource::from_bytes(text.to_owned());
    let index = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
    let blocks = (0..index.pages.len())
        .flat_map(|page| index.parse_page(&source, page, || false).unwrap())
        .collect();
    (index, blocks)
}

#[test]
fn compact_records_and_small_document_parity() {
    assert_eq!(size_of::<BlockEntry>(), 32);
    for text in [
        include_str!("../../../docs/fixtures/tables.md"),
        include_str!("../../../docs/fixtures/safe-html.md"),
        include_str!("../../../docs/fixtures/math-headings.md"),
        "# Title\n\n**Hello** *world* &amp; café\n\n> quote\n>\n> - nested\n>   - deeper\n\n---\n",
        "---\ntitle: café\nitems: [one, two]\n---\n# Body\n\n- [x] done\n- next\n\n```rust\nlet x = 42;\n```\n",
        "+++\nname = 'test'\n[table]\na = 1\n+++\n# Body\n\nTerm\n: definition\n",
        "[link][ID] and ![image][picture]\n\n[ID]: https://example.com \"title\"\n[picture]: image.svg\n",
        "Before[^note].\n\n[^note]: a **footnote**.\n\n## End\n",
        "| A | B |\n| :-- | --: |\n| *a* | b |\n\n$$x^2$$\n",
        "<div>\nhello\n</div>\n\n<!-- hidden -->\n\nUnicode café 😀\r\n\r\nEnd\r\n",
    ] {
        let eager = parse(text);
        let (index, blocks) = indexed(text);
        assert_eq!(blocks, eager.blocks, "{text}");
        assert_eq!(index.headings, eager.headings, "{text}");
    }
}

#[test]
fn math_with_blank_lines_stays_whole_across_page_windows() {
    let formula = format!(
        "$$\n\\begin{{aligned}}\n{}\n\\end{{aligned}}\n$$",
        "a &= b \\\\\n\n".repeat(800)
    );
    let text = format!(
        "{}\n{formula}\n\n# After\n",
        "padding\n\n".repeat(PAGE_BYTES / 9 - 40)
    );
    let eager = parse(&text);
    let (index, blocks) = indexed(&text);
    assert!(index.pages.len() > 1);
    assert_eq!(blocks, eager.blocks);
    assert_eq!(index.headings, eager.headings);
    let math: Vec<_> = blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Math)
        .collect();
    assert_eq!(math.len(), 1);
    assert_eq!(&text[math[0].source.clone()], formula);
    assert!(
        index
            .pages
            .iter()
            .all(|page| page.source.len() <= PARSE_WINDOW_BYTES)
    );
}

#[test]
fn late_definitions_duplicate_headings_and_rendered_find_across_pages() {
    let mut text = String::from("# Same\n\n[forward][ID] and note[^N].\n\n");
    text.push_str(&"**Hello** *world* &amp; café.\n\n".repeat(5000));
    text.push_str("# Same\n\n[ID]: https://example.com/first\n\n[^N]: footnote\n\n");
    text.push_str(&"padding text\n\n".repeat(5000));
    text.push_str("# Same\n\n[forward][id]\n\n[ID]: https://example.com/later\n");
    let eager = parse(&text);
    let (index, blocks) = indexed(&text);
    assert!(index.pages.len() > 2);
    assert_eq!(blocks, eager.blocks);
    assert_eq!(index.headings, eager.headings);
    let source = FileSource::from_bytes(text);
    let mut hits = Vec::new();
    index
        .search(
            &source,
            "Hello world & café",
            10_000,
            || false,
            |batch| {
                hits.extend(batch);
                true
            },
        )
        .unwrap();
    assert_eq!(
        hits,
        find_rendered(&eager.blocks, 0, "Hello world & café", 10_000)
    );
}

#[test]
fn large_tables_and_fences_keep_context_and_every_byte() {
    let row = "| café | **bold** &amp; |\r\n";
    let code = "let value = 42; // café\r\n";
    let text = format!(
        "| A | B |\r\n| :-- | --: |\r\n{}\r\n```rust\r\n{}```\r\n\r\n# End\r\n",
        row.repeat(20_000),
        code.repeat(20_000)
    );
    let (index, blocks) = indexed(&text);
    assert!(index.warnings.is_empty(), "{:?}", index.warnings);
    assert_eq!(
        index
            .blocks
            .iter()
            .filter(|b| b.kind == BlockType::TableHead)
            .count(),
        1
    );
    assert_eq!(
        index
            .blocks
            .iter()
            .filter(|b| b.kind == BlockType::TableRow)
            .count(),
        20_000
    );
    assert_eq!(
        blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::TableRow { header: true, .. }))
            .count(),
        1
    );
    assert_eq!(
        blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::TableRow { header: false, .. }))
            .count(),
        20_000
    );
    let code_blocks: Vec<_> = blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Code { .. }))
        .collect();
    assert_eq!(
        code_blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Code { first: true, .. }))
            .count(),
        1
    );
    assert_eq!(
        code_blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<String>(),
        code.repeat(20_000).replace("\r\n", "\n")
    );
    assert!(
        code_blocks
            .iter()
            .all(|b| matches!(&b.kind, BlockKind::Code { language, .. } if language == "rust"))
    );
    assert_eq!(index.headings.last().unwrap().text, "End");
    assert!(
        index
            .pages
            .iter()
            .all(|p| p.source.len() <= PARSE_WINDOW_BYTES)
    );
    for block in &blocks {
        assert!(text.is_char_boundary(block.source.start));
        assert!(text.is_char_boundary(block.source.end));
        assert!(block.source.end <= text.len());
    }
}

#[test]
fn cancelled_work_and_modified_source_are_rejected() {
    let source = FileSource::from_bytes("hello");
    assert_eq!(
        DocumentIndex::build(&source, || true, |_, _| 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    let index = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
    assert!(
        index
            .parse_page(&FileSource::from_bytes("other"), 0, || false)
            .is_err()
    );
    assert!(
        index
            .search(&source, "hello", 10, || true, |_| panic!("stale results"))
            .is_err()
    );
}

#[test]
fn byte_cache_evicts_and_search_does_not_retain_pages() {
    let source = FileSource::from_bytes("paragraph **bold** text\n\n".repeat(30_000));
    let index = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
    let mut cache = PageCache::new(2 * 1024 * 1024);
    for _ in 0..2 {
        for page in (0..index.pages.len()).rev() {
            assert!(cache.insert(page, index.parse_page(&source, page, || false).unwrap()));
            assert!(cache.bytes() <= 2 * 1024 * 1024);
        }
    }
    assert!(cache.len() < index.pages.len());
    let before = cache.bytes();
    let mut hits = 0;
    index
        .search(
            &source,
            "paragraph bold",
            123,
            || false,
            |batch| {
                hits += batch.len();
                true
            },
        )
        .unwrap();
    assert_eq!(hits, 123);
    assert_eq!(cache.bytes(), before);
    assert!(index.retained_bytes() < source.len() as usize * 2);
}

#[test]
fn page_boundaries_preserve_containers_setext_and_reference_titles() {
    let constructs = [
        "1. first\n\n   paragraph within item\n\n   > nested quote\n   > - one\n   > - two\n\n2. next\n\n",
        "Setext **heading**\n==================\n\n[ref][Straße]\n\n[STRASSE]: /first\n  \"multiline title\"\n\n",
        "    indented code\n\n    more code\n\n<div>\nHTML block\n</div>\n\n",
        "- list\n  ```text\n  nested fence\n  ```\n\n> quote\n>\n> continued\n\n",
    ];
    for construct in constructs {
        for shift in [0, 31, 1021] {
            let text = format!(
                "{}{}{}",
                "padding\n\n".repeat(7200 + shift),
                construct.repeat(50),
                "end\n\n".repeat(1000)
            );
            let eager = parse(&text);
            let (index, blocks) = indexed(&text);
            assert_eq!(blocks.len(), eager.blocks.len());
            for (i, (actual, expected)) in blocks.iter().zip(&eager.blocks).enumerate() {
                assert_eq!(
                    actual, expected,
                    "block {i}, shift {shift}, construct {construct}"
                );
            }
            assert_eq!(index.headings, eager.headings);
        }
    }
}

#[test]
fn equation_prefixes_are_independent_of_page_visit_order() {
    let text = format!(
        "$$one$$\n\n{}\n\n$$two$$\n\n{}\n\n$$one$$\n",
        "padding\n\n".repeat(10_000),
        "more padding\n\n".repeat(10_000)
    );
    let source = FileSource::from_bytes(text);
    let index =
        DocumentIndex::build(&source, || false, |tex, _| if tex == "two" { 2 } else { 1 }).unwrap();
    let mut numbers: Vec<_> = index.equation_starts.iter().collect();
    numbers.sort_by_key(|(block, _)| **block);
    assert_eq!(
        numbers.iter().map(|(_, n)| **n).collect::<Vec<_>>(),
        [1, 2, 4]
    );
    for page in (0..index.pages.len()).rev() {
        index.parse_page(&source, page, || false).unwrap();
    }
    assert_eq!(index.equation_starts.len(), 3);
}

#[test]
fn long_unterminated_code_and_oversized_lines_are_bounded_and_explicit() {
    let text = format!("```\n{}", "a short code line\n".repeat(30_000));
    let (index, blocks) = indexed(&text);
    assert!(index.warnings.is_empty());
    assert_eq!(
        blocks.iter().map(|b| b.text.as_str()).collect::<String>(),
        "a short code line\n".repeat(30_000)
    );
    let text = "é".repeat(300_000);
    let (index, blocks) = indexed(&text);
    assert!(!index.warnings.is_empty());
    assert!(
        index
            .pages
            .iter()
            .all(|p| p.source.len() <= PARSE_WINDOW_BYTES)
    );
    assert_eq!(
        blocks.iter().map(|b| b.text.as_str()).collect::<String>(),
        text
    );
}

#[test]
fn footnote_context_does_not_inject_text_into_open_code_or_comments() {
    for end in ["```\n[^note]", "<!-- [^note]", "<div>\n[^note]"] {
        let text = format!(
            "[^note]: real note\n\n{}\n\n{end}",
            "padding\n\n".repeat(10_000)
        );
        let eager = parse(&text);
        let (_, blocks) = indexed(&text);
        for (i, (actual, expected)) in blocks.iter().zip(&eager.blocks).enumerate() {
            assert_eq!(actual, expected, "ending {end}, block {i}");
        }
        assert_eq!(blocks.len(), eager.blocks.len(), "ending {end}");
    }
}

#[test]
fn a_long_code_line_preserves_text_or_uses_explicit_source_fallback() {
    let text = format!("```rust\n{}\n```\n\n# End\n", "x".repeat(500_000));
    let (index, blocks) = indexed(&text);
    if index.warnings.is_empty() {
        let code: String = blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Code { .. }))
            .map(|b| b.text.as_str())
            .collect();
        assert_eq!(code.len(), 500_001);
    } else {
        assert!(
            index
                .pages
                .iter()
                .all(|p| p.source.len() <= PARSE_WINDOW_BYTES)
        );
    }
}

#[test]
fn oversized_table_header_has_an_explicit_bounded_fallback() {
    let text = format!(
        "| {} | B |\n| -- | -- |\n| one | two |\n\n# End\n",
        "x".repeat(100_000)
    );
    let (index, blocks) = indexed(&text);
    assert!(!index.warnings.is_empty());
    assert!(
        index
            .pages
            .iter()
            .all(|p| p.source.len() <= PARSE_WINDOW_BYTES)
    );
    assert!(blocks.iter().all(|b| b.text.len() <= BLOCK_TEXT_LIMIT));
}

#[test]
fn disclosure_parent_indices_preserve_offsets_past_four_gib() {
    let start = u32::MAX as usize + 1024;
    let header = Block::new(BlockKind::Details { open: false }, start..start + 9);
    let mut body = Block::new(BlockKind::Paragraph, start + 10..start + 20);
    body.details = Some(start);
    let document = Document {
        blocks: vec![Arc::new(header), Arc::new(body)],
        ..Document::default()
    };
    let index = DocumentIndex::initial(&document, start + 20);
    assert_eq!(size_of::<BlockEntry>(), 32);
    assert_eq!(
        BlockEntry::ancestors(&index.blocks, 1).collect::<Vec<_>>(),
        [start]
    );
    assert!(!BlockEntry::visible(&index.blocks, 1, &Default::default()));
    assert!(BlockEntry::visible(
        &index.blocks,
        1,
        &std::collections::HashSet::from([start])
    ));
}

#[test]
fn indexes_and_reads_immutable_unsaved_versions_without_filling_the_edit_cache() {
    use plastext_buffer::{EditBuffer, OpenOptions, UndoGroup};
    use std::sync::atomic::AtomicBool;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged.md");
    let raw = format!(
        "# Original\n\n{}[ref]: https://example.com\n",
        "paragraph [link][ref]\n\n".repeat(50_000)
    );
    std::fs::write(&path, &raw).unwrap();
    let mut buffer = EditBuffer::open(
        &path,
        OpenOptions {
            large_file_threshold: 1,
            block_cache: 1024 * 1024,
        },
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    buffer.snapshot().load(0..64).unwrap();
    buffer
        .replace(2..10, b"Unsaved title", UndoGroup::New)
        .unwrap();
    let snapshot = buffer.snapshot();
    let distant = snapshot.len() - 64..snapshot.len();
    assert!(snapshot.missing(distant.clone()).is_some());
    let index = DocumentIndex::build(&snapshot, || false, |_, _| 0).unwrap();
    assert_eq!(index.headings[0].text, "Unsaved title");
    assert!(index.pages.len() > 1);
    buffer
        .replace(2..15, b"Newer title", UndoGroup::New)
        .unwrap();
    let page = index.parse_page(&snapshot, 0, || false).unwrap();
    assert_eq!(page[0].text, "Unsaved title");
    assert!(
        page.iter()
            .flat_map(|b| &b.spans)
            .any(|span| span.style.link.as_deref() == Some("https://example.com"))
    );
    assert!(
        snapshot.missing(distant).is_some(),
        "preview scanning bypasses the editor cache"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
}
