//! Static main-line positions for Markdown fences. Parsing and rasterization
//! use the same bounded native backend as the standalone Go viewer.
use document_graphics::{
    Raster, RasterOptions,
    sgf::{self, Collection, Error, NodeId},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const MAX_NODES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    /// One-based game within the collection.
    game: usize,
    /// Main-line move ordinal, including passes. None selects its final node;
    /// zero selects the root, including its setup stones and any root move.
    at_move: Option<usize>,
}
impl Selection {
    fn parse(attributes: &str) -> Result<Self, Error> {
        let invalid = || {
            Error::Unsupported("Invalid SGF selectors: use game=1 and move=last (or a move number)")
        };
        let mut selection = Self {
            game: 1,
            at_move: None,
        };
        let mut game = false;
        let mut at_move = false;
        for word in attributes.split_whitespace() {
            let (key, value) = word.split_once('=').ok_or_else(invalid)?;
            match key {
                "game" if !game => {
                    selection.game = value.parse().map_err(|_| invalid())?;
                    game = true;
                }
                "move" if !at_move => {
                    selection.at_move = if value == "last" {
                        None
                    } else {
                        Some(value.parse().map_err(|_| invalid())?)
                    };
                    at_move = true;
                }
                _ => return Err(invalid()),
            }
        }
        if selection.game == 0 {
            return Err(invalid());
        }
        Ok(selection)
    }

    fn node(self, collection: &Collection, cancel: &AtomicBool) -> Result<(NodeId, usize), Error> {
        let mut node = collection
            .games()
            .get(self.game - 1)
            .ok_or(Error::Unsupported("SGF game is out of range"))?
            .root;
        let mut moves = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if collection.text(node, "B").is_some() || collection.text(node, "W").is_some() {
                moves += 1;
            }
            if self.at_move == Some(0) || self.at_move == Some(moves) {
                return Ok((node, moves));
            }
            match collection.nodes()[node].first_child {
                Some(child) => node = child,
                None if self.at_move.is_none() => return Ok((node, moves)),
                None => return Err(Error::Unsupported("SGF move is out of range")),
            }
        }
    }
}

/// Called only in a background job. The parsed collection and board position
/// are dropped after rendering; the view retains only a budgeted raster.
pub(super) fn render(
    source: &str,
    attributes: &str,
    light: bool,
    cancel: &AtomicBool,
) -> Result<(Raster, String), Error> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if source.len() > plastext_markdown::BLOCK_TEXT_LIMIT {
        return Err(Error::Limit("SGF fence exceeds 16 KiB"));
    }
    let selection = Selection::parse(attributes)?;
    let collection = Collection::parse(Arc::from(source.as_bytes()), cancel)?;
    if collection.nodes().len() > MAX_NODES {
        return Err(Error::Limit("SGF fence exceeds 8192 nodes"));
    }
    let (node, moves) = selection.node(&collection, cancel)?;
    let position = collection.position(node, cancel)?;
    let pixels = sgf::rasterize(
        &position,
        sgf::RenderOptions {
            dark: !light,
            move_numbers: false,
        },
        RasterOptions {
            bounds: (1000, 1000),
            scale: 2.,
        },
        cancel,
    )?;
    Ok((
        pixels,
        format!(
            "Game {}/{} · move {moves} · main line",
            selection.game,
            collection.games().len()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str =
        "(;FF[4]GM[1]SZ[9]AB[aa];B[bb];C[Note](;W[];B[cc]MN[20])(;W[dd]))(;GM[1]SZ[9];B[ee])";

    #[test]
    fn selectors_follow_main_line_move_ordinals_and_count_passes() {
        let cancel = AtomicBool::new(false);
        let collection = Collection::parse(Arc::from(SOURCE.as_bytes()), &cancel).unwrap();
        for (attributes, property, value, moves) in [
            ("", "B", "cc", 3),
            ("move=last", "B", "cc", 3),
            ("move=0", "AB", "aa", 0),
            ("move=1", "B", "bb", 1),
            ("move=2", "W", "", 2),
            ("game=2", "B", "ee", 1),
        ] {
            let (node, count) = Selection::parse(attributes)
                .unwrap()
                .node(&collection, &cancel)
                .unwrap();
            assert_eq!(count, moves);
            assert_eq!(collection.text(node, property).as_deref(), Some(value));
        }
        for attributes in ["move=4", "game=3", "move=20"] {
            assert!(
                Selection::parse(attributes)
                    .unwrap()
                    .node(&collection, &cancel)
                    .is_err()
            );
        }
        for attributes in [
            "game=0",
            "move=-1",
            "game=1 game=2",
            "move=last move=0",
            "moves=2",
            "move=",
            "extra",
        ] {
            assert!(Selection::parse(attributes).is_err(), "{attributes}");
        }
        let root_move =
            Collection::parse(Arc::from(b"(;GM[1]SZ[9]B[cc];W[dd])".as_slice()), &cancel).unwrap();
        for attributes in ["move=0", "move=1"] {
            assert_eq!(
                Selection::parse(attributes)
                    .unwrap()
                    .node(&root_move, &cancel)
                    .unwrap(),
                (root_move.games()[0].root, 1)
            );
        }
    }

    #[test]
    fn fixture_boards_render_in_both_appearances_and_invalid_records_keep_source() {
        let cancel = AtomicBool::new(false);
        let document = plastext_markdown::parse(include_str!("../../../../docs/fixtures/sgf.md"));
        let fences: Vec<_> = document
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                plastext_markdown::BlockKind::Code { attributes, .. } => {
                    Some((&block.text, attributes))
                }
                _ => None,
            })
            .collect();
        assert_eq!(fences.len(), 6);
        for light in [false, true] {
            for (i, (source, attributes)) in fences.iter().enumerate() {
                let result = render(source, attributes, light, &cancel);
                if i >= 4 {
                    assert!(result.is_err());
                } else {
                    let (pixels, caption) = result.unwrap();
                    assert_eq!(pixels.width, pixels.height);
                    assert!(pixels.width > 0 && pixels.width <= 1000);
                    assert_eq!(
                        pixels.rgba.len(),
                        pixels.width as usize * pixels.height as usize * 4
                    );
                    assert_eq!(
                        caption,
                        [
                            "Game 1/1 · move 3 · main line",
                            "Game 1/1 · move 1 · main line",
                            "Game 1/1 · move 0 · main line",
                            "Game 2/2 · move 2 · main line",
                        ][i]
                    );
                }
            }
        }
        let (light, _) = render(SOURCE, "", true, &cancel).unwrap();
        let (dark, _) = render(SOURCE, "", false, &cancel).unwrap();
        assert_ne!(light.rgba, dark.rgba);
    }

    #[test]
    fn bounded_work_rejects_large_or_cancelled_requests() {
        let cancel = AtomicBool::new(false);
        let oversized = format!("(;C[{}])", "x".repeat(plastext_markdown::BLOCK_TEXT_LIMIT));
        let many_nodes = format!("(;{})", ";".repeat(MAX_NODES));
        for source in [&oversized, &many_nodes] {
            assert!(matches!(
                render(source, "", true, &cancel),
                Err(Error::Limit(_))
            ));
        }
        for source in ["broken", "(;GM[2]SZ[9])"] {
            assert!(render(source, "", false, &cancel).is_err());
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            render(SOURCE, "", true, &cancel),
            Err(Error::Cancelled)
        ));
    }
}
