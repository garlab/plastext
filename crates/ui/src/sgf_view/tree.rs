use document_graphics::sgf::{Collection, Color, NodeId};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(super) struct Node {
    pub row: usize,
    pub column: usize,
    pub number: u32,
    pub color: Option<Color>,
    pub comment: bool,
    pub last_child: Option<NodeId>,
}
#[derive(Debug)]
pub(super) struct Game {
    pub rows: usize,
    pub columns: usize,
    range: Range<usize>,
}
/// One compact entry per source node. Geometry is indexed once off the UI thread;
/// neither board positions nor text layouts are retained for tree nodes.
pub(super) struct Tree {
    pub nodes: Vec<Node>,
    pub games: Vec<Game>,
    by_row: Vec<NodeId>,
}
impl Tree {
    pub fn new(document: &Collection) -> Self {
        let mut nodes: Vec<Node> = Vec::with_capacity(document.nodes().len());
        let mut games: Vec<Game> = document
            .games()
            .iter()
            .map(|_| Game {
                rows: 0,
                columns: 1,
                range: 0..0,
            })
            .collect();
        for (id, node) in document.nodes().iter().enumerate() {
            let game = &mut games[node.game];
            let (row, column, number) = if let Some(parent) = node.parent {
                let previous = nodes[parent];
                let column = if document.nodes()[parent].first_child == Some(id) {
                    previous.column
                } else {
                    let column = game.columns;
                    game.columns += 1;
                    column
                };
                (previous.row + 1, column, previous.number)
            } else {
                (0, 0, 0)
            };
            let color = if document.text(id, "B").is_some() {
                Some(Color::Black)
            } else if document.text(id, "W").is_some() {
                Some(Color::White)
            } else {
                None
            };
            let number = if color.is_some() {
                document
                    .text(id, "MN")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(number.saturating_add(1))
            } else {
                number
            };
            nodes.push(Node {
                row,
                column,
                number,
                color,
                comment: document.text(id, "C").is_some_and(|s| !s.is_empty()),
                last_child: None,
            });
            if let Some(parent) = node.parent {
                nodes[parent].last_child = Some(id);
            }
            game.rows = game.rows.max(row + 1);
            if node.parent.is_none() {
                game.range.start = id;
            }
            game.range.end = id + 1;
        }
        let mut by_row: Vec<_> = (0..nodes.len()).collect();
        by_row.sort_unstable_by_key(|&id| {
            (document.nodes()[id].game, nodes[id].row, nodes[id].column)
        });
        Self {
            nodes,
            games,
            by_row,
        }
    }
    /// Visit only the requested rows and columns, including very broad branches.
    pub fn visible(&self, game: usize, rows: Range<usize>, columns: Range<usize>) -> Vec<NodeId> {
        if rows.is_empty() || columns.is_empty() {
            return Vec::new();
        }
        let sorted = &self.by_row[self.games[game].range.clone()];
        let begin = sorted.partition_point(|&id| self.nodes[id].row < rows.start);
        let end = sorted.partition_point(|&id| self.nodes[id].row < rows.end);
        let mut remaining = &sorted[begin..end];
        let mut result = Vec::new();
        while let Some(&first) = remaining.first() {
            let end = remaining.partition_point(|&id| self.nodes[id].row == self.nodes[first].row);
            let row = &remaining[..end];
            let left = row.partition_point(|&id| self.nodes[id].column < columns.start);
            let right = row.partition_point(|&id| self.nodes[id].column < columns.end);
            result.extend_from_slice(&row[left..right]);
            remaining = &remaining[end..];
        }
        result
    }
    pub fn at(&self, game: usize, row: usize, column: usize) -> Option<NodeId> {
        self.visible(game, row..row + 1, column..column + 1)
            .first()
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};
    fn parse(text: &str) -> Collection {
        Collection::parse(Arc::from(text.as_bytes()), &AtomicBool::new(false)).unwrap()
    }
    #[test]
    fn branches_are_side_by_side_with_stable_move_numbers_and_game_ranges() {
        let doc =
            parse("(;SZ[9];B[aa](;W[bb]C[Main];B[]MN[10])(;W[cc](;B[dd])(;B[ee])))(;SZ[13];W[])");
        let tree = Tree::new(&doc);
        assert_eq!(tree.visible(0, 2..3, 0..9), vec![2, 4]);
        assert_eq!(tree.visible(0, 3..4, 0..9), vec![3, 5, 6]);
        assert_eq!(tree.nodes[3].number, 10);
        assert_eq!(tree.nodes[6].number, 3);
        assert!(tree.nodes[2].comment);
        assert_eq!(tree.games[0].columns, 3);
        assert_eq!(tree.visible(1, 0..9, 0..9), vec![7, 8]);
        assert_eq!(tree.nodes[8].number, 1);
        assert!(tree.visible(0, 2..2, 0..9).is_empty());
        assert!(tree.visible(0, 0..9, 2..2).is_empty());
        for game in 0..tree.games.len() {
            for id in tree.visible(game, 0..100, 0..100) {
                let n = tree.nodes[id];
                assert_eq!(tree.at(game, n.row, n.column), Some(id));
            }
        }
    }
    #[test]
    fn large_trees_query_only_visible_nodes() {
        let doc = parse(&format!("(;SZ[9]{})", ";B[];W[]".repeat(20_000)));
        let tree = Tree::new(&doc);
        assert_eq!(tree.games[0].columns, 1);
        assert_eq!(
            tree.visible(0, 20_000..20_010, 0..5),
            (20_000..20_010).collect::<Vec<_>>()
        );
        let doc = parse(&format!("(;SZ[9]{})", "(;B[aa])".repeat(10_000)));
        let tree = Tree::new(&doc);
        assert_eq!(
            tree.visible(0, 1..2, 5000..5004),
            vec![5001, 5002, 5003, 5004]
        );
    }
}
