use super::tree::Tree;
use document_graphics::sgf::{Collection, Error, NodeId, Position};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, atomic::AtomicBool},
};

/// Remember the selected branch without materializing a board for every move.
#[derive(Clone)]
pub(super) struct Cursor {
    pub document: Arc<Collection>,
    pub selected: NodeId,
    choices: HashMap<NodeId, NodeId>,
    pub tree: Arc<Tree>,
    pub path: Arc<Vec<bool>>,
}
impl Cursor {
    pub fn new(document: Arc<Collection>) -> Self {
        let mut path = vec![false; document.nodes().len()];
        path[document.games()[0].root] = true;
        Self {
            tree: Arc::new(Tree::new(&document)),
            path: Arc::new(path),
            selected: document.games()[0].root,
            document,
            choices: HashMap::new(),
        }
    }
    pub fn select(&mut self, node: NodeId) {
        let path = Arc::make_mut(&mut self.path);
        let mut old = Some(self.selected);
        while let Some(id) = old {
            path[id] = false;
            old = self.document.nodes()[id].parent;
        }
        let mut current = Some(node);
        while let Some(id) = current {
            path[id] = true;
            current = self.document.nodes()[id].parent;
        }
        self.selected = node;
        let mut child = node;
        while let Some(parent) = self.document.nodes()[child].parent {
            if self.is_fork(parent) {
                self.choices.insert(parent, child);
            }
            child = parent;
        }
    }
    pub fn next(&self) -> Option<NodeId> {
        self.next_from(self.selected)
    }
    fn next_from(&self, node: NodeId) -> Option<NodeId> {
        self.choices
            .get(&node)
            .copied()
            .or(self.document.nodes()[node].first_child)
    }
    fn is_fork(&self, node: NodeId) -> bool {
        self.document.nodes()[node]
            .first_child
            .is_some_and(|id| self.document.nodes()[id].next_sibling.is_some())
    }
    pub fn previous(&self) -> Option<NodeId> {
        self.document.nodes()[self.selected].parent
    }
    pub fn game(&self) -> usize {
        self.document.nodes()[self.selected].game
    }
    pub fn first(&self) -> NodeId {
        self.document.games()[self.game()].root
    }
    pub fn last(&self) -> NodeId {
        let mut node = self.selected;
        while let Some(next) = self.next_from(node) {
            node = next;
        }
        node
    }
    pub fn variation(&self, direction: i32) -> NodeId {
        self.tree.variation(self.game(), self.selected, direction)
    }
    pub fn previous_fork(&self) -> Option<NodeId> {
        let mut node = self.previous()?;
        while !self.is_fork(node) {
            let Some(parent) = self.document.nodes()[node].parent else {
                break;
            };
            node = parent;
        }
        Some(node)
    }
    pub fn next_fork(&self) -> Option<NodeId> {
        let mut node = self.next()?;
        while !self.is_fork(node) {
            let Some(next) = self.next_from(node) else {
                break;
            };
            node = next;
        }
        Some(node)
    }
}

/// Small board snapshots, never a bitmap or a position for every game node.
#[derive(Clone, Default)]
pub(super) struct Positions(VecDeque<Arc<Position>>);
impl Positions {
    pub fn remember(&mut self, position: Arc<Position>) {
        self.0.retain(|p| p.node() != position.node());
        self.0.push_back(position);
        while self.0.len() > 32 {
            self.0.pop_front();
        }
    }
    pub fn get(
        &self,
        doc: &Collection,
        node: NodeId,
        cancel: &AtomicBool,
    ) -> Result<Arc<Position>, Error> {
        if let Some(p) = self.0.iter().find(|p| p.node() == node) {
            return Ok(p.clone());
        }
        if let Some(parent) = doc.nodes()[node].parent
            && let Some(p) = self.0.iter().find(|p| p.node() == parent)
        {
            return p.advance(doc, node, cancel).map(Arc::new);
        }
        doc.position(node, cancel).map(Arc::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> Arc<Collection> {
        Arc::new(
            Collection::parse(
                Arc::from(&b"(;SZ[9];B[aa](;W[bb];B[cc])(;W[dd];B[ee]))(;SZ[13])"[..]),
                &AtomicBool::new(false),
            )
            .unwrap(),
        )
    }
    #[test]
    fn navigation_remembers_variations_and_splits_have_independent_cursors() {
        let mut cursor = Cursor::new(document());
        assert_eq!(cursor.last(), 3);
        cursor.select(4);
        assert_eq!(cursor.variation(-1), 2);
        let split = cursor.clone();
        cursor.select(cursor.previous().unwrap());
        assert_eq!(cursor.next(), Some(4));
        assert_eq!(cursor.last(), 5);
        assert_eq!(split.selected, 4);
        assert!(Arc::ptr_eq(&split.tree, &cursor.tree));
        assert!(split.path[4]);
        assert!(!cursor.path[4]);
        assert!(cursor.path[0] && cursor.path[1]);
        cursor.select(6);
        assert_eq!(cursor.first(), 6);
        assert_eq!(cursor.next(), None);
    }
    #[test]
    fn fork_navigation_skips_current_fork_and_remembers_the_chosen_line() {
        let doc = Collection::parse(
            Arc::from(
                &b"(;SZ[9];B[aa](;W[bb];B[cc])(;W[dd];B[ee](;W[ff])(;W[gg];B[hh])))(;SZ[13])"[..],
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut cursor = Cursor::new(Arc::new(doc));
        assert_eq!(cursor.previous_fork(), None);
        assert_eq!(cursor.next_fork(), Some(1));
        cursor.select(1);
        assert_eq!(cursor.previous_fork(), Some(0));
        assert_eq!(cursor.next_fork(), Some(3)); // Main line ends without another fork.
        cursor.select(3);
        assert_eq!(cursor.next_fork(), None);
        assert_eq!(cursor.previous_fork(), Some(1));
        cursor.select(8); // Select the second branch at both forks.
        assert_eq!(cursor.previous_fork(), Some(5));
        cursor.select(5);
        assert_eq!(cursor.previous_fork(), Some(1));
        assert_eq!(cursor.next_fork(), Some(8));
        cursor.select(1);
        assert_eq!(cursor.next_fork(), Some(5));
        assert_eq!(cursor.last(), 8);
        cursor.select(9); // Another game must remain independent.
        assert_eq!(cursor.previous_fork(), None);
        assert_eq!(cursor.next_fork(), None);
    }
    #[test]
    fn horizontal_navigation_follows_tree_rows_and_remembers_the_new_branch() {
        let mut cursor = Cursor::new(document());
        cursor.select(3);
        assert_eq!(cursor.variation(1), 5); // Different parents, same depth.
        cursor.select(cursor.variation(1));
        assert_eq!(cursor.variation(1), 3); // Wrap at the end of the row.
        assert_eq!(cursor.variation(-1), 3);
        cursor.select(cursor.first());
        assert_eq!(cursor.last(), 5);
        assert_eq!(cursor.variation(1), 0); // Don't enter another game's root.
        cursor.select(6);
        assert_eq!(cursor.variation(-1), 6);
    }
    #[test]
    fn cached_branch_positions_are_isolated_and_bounded() {
        let doc = document();
        let cancel = AtomicBool::new(false);
        let mut cache = Positions::default();
        cache.remember(cache.get(&doc, 1, &cancel).unwrap());
        let main = cache.get(&doc, 2, &cancel).unwrap();
        let branch = cache.get(&doc, 4, &cancel).unwrap();
        assert!(
            main.stone(document_graphics::sgf::Point { x: 1, y: 1 })
                .is_some()
        );
        assert!(
            branch
                .stone(document_graphics::sgf::Point { x: 1, y: 1 })
                .is_none()
        );
        let long = format!("(;SZ[9]{})", ";B[];W[]".repeat(40));
        let doc = Collection::parse(Arc::from(long.as_bytes()), &cancel).unwrap();
        let mut cache = Positions::default();
        for node in 0..doc.nodes().len() {
            cache.remember(cache.get(&doc, node, &cancel).unwrap());
        }
        assert_eq!(cache.0.len(), 32);
    }
}
