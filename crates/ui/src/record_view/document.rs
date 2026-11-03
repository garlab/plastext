//! Adapt Go and chess records to the same navigation, cache and view lifecycle.
use document_graphics::{Raster, RasterOptions, chess, sgf};
pub(super) use sgf::{Color, NodeId};
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Go,
    Pgn,
    Fen,
}
impl Kind {
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "sgf" => Some(Self::Go),
            "pgn" => Some(Self::Pgn),
            "fen" => Some(Self::Fen),
            _ => None,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Go => "Go record",
            Self::Pgn => "Chess record",
            Self::Fen => "Chess position",
        }
    }
    pub fn format(self) -> &'static str {
        match self {
            Self::Go => "SGF",
            Self::Pgn => "PGN",
            Self::Fen => "FEN",
        }
    }
    pub fn option(self) -> &'static str {
        if self == Self::Go {
            "Numbers"
        } else {
            "Flip board"
        }
    }
    pub fn max_source(self) -> usize {
        if self == Self::Go {
            sgf::MAX_SOURCE
        } else {
            chess::MAX_SOURCE
        }
    }
}
#[derive(Debug)]
pub(super) enum Collection {
    Go(sgf::Collection),
    Chess(chess::Collection),
}
pub(super) struct Node {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub source: Range<usize>,
    pub game: usize,
}
impl Collection {
    pub fn parse(source: Arc<[u8]>, kind: Kind, cancel: &AtomicBool) -> Result<Self, String> {
        match kind {
            Kind::Go => sgf::Collection::parse(source, cancel)
                .map(Self::Go)
                .map_err(|e| e.to_string()),
            Kind::Pgn | Kind::Fen => chess::Collection::parse(
                source,
                if kind == Kind::Pgn {
                    chess::Format::Pgn
                } else {
                    chess::Format::Fen
                },
                cancel,
            )
            .map(Self::Chess)
            .map_err(|e| e.to_string()),
        }
    }
    pub fn node_count(&self) -> usize {
        match self {
            Self::Go(d) => d.nodes().len(),
            Self::Chess(d) => d.nodes().len(),
        }
    }
    pub fn node(&self, id: NodeId) -> Node {
        match self {
            Self::Go(d) => {
                let n = &d.nodes()[id];
                Node {
                    parent: n.parent,
                    first_child: n.first_child,
                    next_sibling: n.next_sibling,
                    source: n.source.clone(),
                    game: n.game,
                }
            }
            Self::Chess(d) => {
                let n = &d.nodes()[id];
                Node {
                    parent: n.parent,
                    first_child: n.first_child,
                    next_sibling: n.next_sibling,
                    source: n.source.clone(),
                    game: n.game,
                }
            }
        }
    }
    pub fn game_count(&self) -> usize {
        match self {
            Self::Go(d) => d.games().len(),
            Self::Chess(d) => d.games().len(),
        }
    }
    pub fn root(&self, game: usize) -> NodeId {
        match self {
            Self::Go(d) => d.games()[game].root,
            Self::Chess(d) => d.games()[game].root,
        }
    }
    pub fn node_for_offset(&self, offset: usize) -> NodeId {
        match self {
            Self::Go(d) => d.node_for_offset(offset),
            Self::Chess(d) => d.node_for_offset(offset),
        }
    }
    pub fn title(&self, game: usize) -> String {
        match self {
            Self::Go(d) => d.title(game),
            Self::Chess(d) => d.title(game),
        }
    }
    pub fn color(&self, id: NodeId) -> Option<Color> {
        match self {
            Self::Go(d) => {
                if d.text(id, "B").is_some() {
                    Some(Color::Black)
                } else if d.text(id, "W").is_some() {
                    Some(Color::White)
                } else {
                    None
                }
            }
            Self::Chess(d) => d.move_color(id).map(|c| {
                if c == chess::Color::Black {
                    Color::Black
                } else {
                    Color::White
                }
            }),
        }
    }
    pub fn number(&self, id: NodeId, previous: u32) -> u32 {
        match self {
            Self::Go(d) => {
                if self.color(id).is_some() {
                    d.text(id, "MN")
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(previous.saturating_add(1))
                } else {
                    previous
                }
            }
            Self::Chess(d) => d.move_number(id),
        }
    }
    pub fn has_comment(&self, id: NodeId) -> bool {
        match self {
            Self::Go(d) => d.text(id, "C").is_some_and(|s| !s.is_empty()),
            Self::Chess(d) => d.has_comment(id),
        }
    }
    pub fn comment(&self, id: NodeId) -> String {
        match self {
            Self::Go(d) => d
                .text(id, "C")
                .or_else(|| {
                    if d.nodes()[id].parent.is_none() {
                        d.text(id, "GC")
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            Self::Chess(d) => [d.annotations(id), d.comment(id)]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
    pub fn comment_title(&self, id: NodeId) -> String {
        match self {
            Self::Go(d) => d
                .text(id, "N")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Comment".into()),
            Self::Chess(d) => d.san(id).map_or_else(
                || "Starting position".into(),
                |san| {
                    format!(
                        "{}{}{san}",
                        d.move_number(id),
                        if d.move_color(id) == Some(chess::Color::Black) {
                            "… "
                        } else {
                            ". "
                        }
                    )
                },
            ),
        }
    }
    pub fn info(&self, id: NodeId, position: Option<&Position>) -> Vec<String> {
        let mut info = Vec::new();
        match self {
            Self::Go(d) => {
                let root = d.games()[d.nodes()[id].game].root;
                for (label, key) in [
                    ("Black", "PB"),
                    ("White", "PW"),
                    ("Result", "RE"),
                    ("Komi", "KM"),
                    ("Date", "DT"),
                ] {
                    if let Some(value) = d.text(root, key).filter(|s| !s.is_empty()) {
                        info.push(format!("{label}: {}", super::clipped(value, 256)));
                    }
                }
                if let Some(Position::Go(p)) = position {
                    info.push(format!(
                        "Captures: Black {} · White {}",
                        p.captures[0], p.captures[1]
                    ));
                }
            }
            Self::Chess(d) => {
                let tags = &d.games()[d.nodes()[id].game].tags;
                for key in [
                    "White", "Black", "Result", "Event", "Site", "Date", "Round", "WhiteElo",
                    "BlackElo", "ECO", "Opening",
                ] {
                    if let Some(value) = tags.get(key).filter(|s| !s.is_empty()) {
                        info.push(format!("{key}: {}", super::clipped(value.clone(), 256)));
                    }
                }
            }
        }
        info
    }
    pub fn position(&self, id: NodeId, cancel: &AtomicBool) -> Result<Position, String> {
        match self {
            Self::Go(d) => d
                .position(id, cancel)
                .map(Position::Go)
                .map_err(|e| e.to_string()),
            Self::Chess(d) => d
                .position(id, cancel)
                .map(Position::Chess)
                .map_err(|e| e.to_string()),
        }
    }
    pub fn tree_label(&self, id: NodeId) -> Option<&str> {
        match self {
            Self::Go(_) => None,
            Self::Chess(d) => d.san(id),
        }
    }
}
#[derive(Debug)]
pub(super) enum Position {
    Go(sgf::Position),
    Chess(chess::Position),
}
impl Position {
    pub fn node(&self) -> NodeId {
        match self {
            Self::Go(p) => p.node(),
            Self::Chess(p) => p.node(),
        }
    }
    pub fn advance(
        &self,
        doc: &Collection,
        id: NodeId,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        match (self, doc) {
            (Self::Go(p), Collection::Go(d)) => p
                .advance(d, id, cancel)
                .map(Self::Go)
                .map_err(|e| e.to_string()),
            (Self::Chess(p), Collection::Chess(d)) => p
                .advance(d, id, cancel)
                .map(Self::Chess)
                .map_err(|e| e.to_string()),
            _ => Err("Record format changed".into()),
        }
    }
    pub fn rasterize(
        &self,
        dark: bool,
        option: bool,
        cancel: &AtomicBool,
    ) -> Result<Raster, String> {
        let fitting = RasterOptions {
            bounds: (1000, 1000),
            scale: 4.,
        };
        match self {
            Self::Go(p) => sgf::rasterize(
                p,
                sgf::RenderOptions {
                    dark,
                    move_numbers: option,
                },
                fitting,
                cancel,
            )
            .map_err(|e| e.to_string()),
            Self::Chess(p) => chess::rasterize(
                p,
                chess::RenderOptions {
                    dark,
                    flipped: option,
                },
                fitting,
                cancel,
            )
            .map_err(|e| e.to_string()),
        }
    }
    pub fn status(&self) -> String {
        match self {
            Self::Go(p) => {
                let last = p
                    .last_move
                    .map(|m| {
                        format!(
                            " · {} {}",
                            if m.color == Color::Black {
                                "Black"
                            } else {
                                "White"
                            },
                            m.point
                                .map_or_else(|| "passes".into(), |pt| pt.coordinate(p.height))
                        )
                    })
                    .unwrap_or_default();
                format!("Move {}{last}", p.move_number)
            }
            Self::Chess(p) => format!(
                "Move {} · {} to move{}",
                p.fullmove_number(),
                if p.side_to_move() == chess::Color::White {
                    "White"
                } else {
                    "Black"
                },
                if p.in_check() { " · Check" } else { "" }
            ),
        }
    }
    pub fn summary(&self) -> String {
        match self {
            Self::Go(p) => format!("{} × {}    Move {}", p.width, p.height, p.move_number),
            Self::Chess(_) => self.status(),
        }
    }
}
