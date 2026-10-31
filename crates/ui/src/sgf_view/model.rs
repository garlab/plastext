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
}
impl Cursor {
    pub fn new(document: Arc<Collection>) -> Self {
        Self {
            selected: document.games()[0].root,
            document,
            choices: HashMap::new(),
        }
    }
    pub fn select(&mut self, node: NodeId) {
        self.selected = node;
        let mut child = node;
        while let Some(parent) = self.document.nodes()[child].parent {
            if self.document.nodes()[parent]
                .first_child
                .is_some_and(|id| self.document.nodes()[id].next_sibling.is_some())
            {
                self.choices.insert(parent, child);
            }
            child = parent;
        }
    }
    pub fn next(&self) -> Option<NodeId> {
        self.choices
            .get(&self.selected)
            .copied()
            .or(self.document.nodes()[self.selected].first_child)
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
        while let Some(next) = self
            .choices
            .get(&node)
            .copied()
            .or(self.document.nodes()[node].first_child)
        {
            node = next;
        }
        node
    }
    pub fn sibling(&self, direction: i32) -> Option<NodeId> {
        let siblings: Vec<_> = self.document.children(self.previous()?).collect();
        let index = siblings.iter().position(|&n| n == self.selected)? as i32;
        siblings
            .get((index + direction).rem_euclid(siblings.len() as i32) as usize)
            .copied()
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
        assert_eq!(cursor.sibling(-1), Some(2));
        let split = cursor.clone();
        cursor.select(cursor.previous().unwrap());
        assert_eq!(cursor.next(), Some(4));
        assert_eq!(cursor.last(), 5);
        assert_eq!(split.selected, 4);
        cursor.select(6);
        assert_eq!(cursor.first(), 6);
        assert_eq!(cursor.next(), None);
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
