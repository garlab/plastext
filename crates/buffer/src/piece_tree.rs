//! Persistent AVL tree of byte ranges. Leaves never exceed 64 KiB, so an
//! edit or line lookup only scans a bounded leaf after its logarithmic descent.
use std::{ops::Range, sync::Arc};

use bytes::{Bytes, BytesMut};
use memchr::memchr_iter;

use crate::unicode_summary::UnicodeSummary;
use crate::{ByteSource, FileSource, ReadError};

pub(super) const LEAF_BYTES: usize = 64 * 1024;
pub(super) type Tree = Option<Arc<Node>>;
const NODE_BYTES: u64 = (size_of::<Node>() + 2 * size_of::<usize>()) as u64;

#[derive(Clone)]
enum Source {
    Original(Arc<FileSource>),
    Added(Bytes),
}

#[derive(Clone)]
pub(super) struct Piece {
    source: Source,
    range: Range<u64>,
    newlines: u64,
}

pub(super) struct Node {
    pub bytes: u64,
    pub newlines: u64,
    pub unicode: UnicodeSummary,
    pub height: u8,
    // Conservative retained size: shared added chunks may be counted twice.
    pub retained: u64,
    kind: Kind,
}

enum Kind {
    Leaf(Piece),
    Branch(Arc<Node>, Arc<Node>),
}

impl Node {
    fn leaf(piece: Piece, unicode: UnicodeSummary) -> Arc<Self> {
        let retained = NODE_BYTES
            + match &piece.source {
                Source::Original(_) => 0,
                Source::Added(bytes) => bytes.len() as u64 + 32,
            };
        Arc::new(Self {
            bytes: piece.range.end - piece.range.start,
            newlines: piece.newlines,
            unicode,
            height: 1,
            retained,
            kind: Kind::Leaf(piece),
        })
    }

    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            bytes: left.bytes + right.bytes,
            newlines: left.newlines + right.newlines,
            unicode: left.unicode.join(left.bytes, right.unicode, right.bytes),
            height: 1 + left.height.max(right.height),
            retained: left.retained + right.retained + NODE_BYTES,
            kind: Kind::Branch(left, right),
        })
    }

    fn children(&self) -> (&Arc<Self>, &Arc<Self>) {
        match &self.kind {
            Kind::Branch(left, right) => (left, right),
            Kind::Leaf(_) => unreachable!("a leaf cannot require a tree rotation"),
        }
    }
}

impl Piece {
    fn len(&self) -> u64 {
        self.range.end - self.range.start
    }

    fn bytes(&self, local: Range<u64>) -> Result<Bytes, ReadError> {
        let source = self.range.start + local.start..self.range.start + local.end;
        match &self.source {
            Source::Original(file) => file.bytes(source).map_err(|error| match error {
                ReadError::Missing(_) => ReadError::Missing(local),
                other => other,
            }),
            Source::Added(bytes) => Ok(bytes.slice(source.start as usize..source.end as usize)),
        }
    }

    pub fn load(&self, local: Range<u64>) -> std::io::Result<()> {
        if let Source::Original(file) = &self.source {
            file.load(self.range.start + local.start..self.range.start + local.end)?;
        }
        Ok(())
    }

    pub fn read_uncached(&self, local: Range<u64>, buffer: &mut [u8]) -> std::io::Result<()> {
        let range = self.range.start + local.start..self.range.start + local.end;
        match &self.source {
            Source::Original(file) => file.read_uncached(range, buffer),
            Source::Added(bytes) => {
                buffer.copy_from_slice(&bytes[range.start as usize..range.end as usize]);
                Ok(())
            }
        }
    }
}

pub(super) fn original(source: Arc<FileSource>, leaves: &[(u64, u64, UnicodeSummary)]) -> Tree {
    let mut offset = 0;
    let pieces = leaves
        .iter()
        .map(|&(len, newlines, unicode)| {
            let range = offset..offset + len;
            offset += len;
            (
                Piece {
                    source: Source::Original(source.clone()),
                    range,
                    newlines,
                },
                unicode,
            )
        })
        .collect::<Vec<_>>();
    build(&pieces)
}

pub(super) fn added(bytes: &[u8]) -> Tree {
    let pieces = bytes
        .chunks(LEAF_BYTES)
        .map(|chunk| {
            (
                Piece {
                    // Own bounded append chunks: a surviving character must not retain an
                    // entire large paste after the rest of that paste is deleted.
                    source: Source::Added(Bytes::copy_from_slice(chunk)),
                    range: 0..chunk.len() as u64,
                    newlines: memchr_iter(b'\n', chunk).count() as u64,
                },
                UnicodeSummary::new(chunk),
            )
        })
        .collect::<Vec<_>>();
    build(&pieces)
}

fn build(pieces: &[(Piece, UnicodeSummary)]) -> Tree {
    match pieces {
        [] => None,
        [(piece, unicode)] => Some(Node::leaf(piece.clone(), *unicode)),
        _ => {
            let middle = pieces.len() / 2;
            Some(Node::branch(
                build(&pieces[..middle]).unwrap(),
                build(&pieces[middle..]).unwrap(),
            ))
        }
    }
}

pub(super) fn len(tree: &Tree) -> u64 {
    tree.as_ref().map_or(0, |node| node.bytes)
}

pub(super) fn cost(tree: &Tree) -> u64 {
    tree.as_ref().map_or(0, |node| node.retained)
}

fn balance(left: Arc<Node>, right: Arc<Node>) -> Arc<Node> {
    if left.height > right.height + 1 {
        let (a, b) = left.children();
        if a.height >= b.height {
            Node::branch(a.clone(), Node::branch(b.clone(), right))
        } else {
            let (b1, b2) = b.children();
            Node::branch(
                Node::branch(a.clone(), b1.clone()),
                Node::branch(b2.clone(), right),
            )
        }
    } else if right.height > left.height + 1 {
        let (a, b) = right.children();
        if b.height >= a.height {
            Node::branch(Node::branch(left, a.clone()), b.clone())
        } else {
            let (a1, a2) = a.children();
            Node::branch(
                Node::branch(left, a1.clone()),
                Node::branch(a2.clone(), b.clone()),
            )
        }
    } else {
        Node::branch(left, right)
    }
}

pub(super) fn join(left: Tree, right: Tree) -> Tree {
    let (left, right) = match (left, right) {
        (None, tree) | (tree, None) => return tree,
        (Some(left), Some(right)) => (left, right),
    };
    Some(if left.height > right.height + 1 {
        let (a, b) = left.children();
        balance(a.clone(), join(Some(b.clone()), Some(right)).unwrap())
    } else if right.height > left.height + 1 {
        let (a, b) = right.children();
        balance(join(Some(left), Some(a.clone())).unwrap(), b.clone())
    } else {
        Node::branch(left, right)
    })
}

fn shifted(error: ReadError, offset: u64) -> ReadError {
    match error {
        ReadError::Missing(range) => ReadError::Missing(range.start + offset..range.end + offset),
        other => other,
    }
}

pub(super) fn split(tree: Tree, offset: u64) -> Result<(Tree, Tree), ReadError> {
    if offset == 0 {
        return Ok((None, tree));
    }
    if offset == len(&tree) {
        return Ok((tree, None));
    }
    let node = tree.unwrap();
    match &node.kind {
        Kind::Leaf(piece) => {
            let bytes = piece.bytes(0..piece.len())?;
            let at = offset as usize;
            let (left_unicode, right_unicode) = node.unicode.split(&bytes, at);
            let newlines = if at <= bytes.len() - at {
                memchr_iter(b'\n', &bytes[..at]).count() as u64
            } else {
                piece.newlines - memchr_iter(b'\n', &bytes[at..]).count() as u64
            };
            let mut left = piece.clone();
            left.range.end = piece.range.start + offset;
            left.newlines = newlines;
            let mut right = piece.clone();
            right.range.start = left.range.end;
            right.newlines -= newlines;
            Ok((
                Some(Node::leaf(left, left_unicode)),
                Some(Node::leaf(right, right_unicode)),
            ))
        }
        Kind::Branch(left, right) => {
            if offset < left.bytes {
                let (before, after) = split(Some(left.clone()), offset)?;
                Ok((before, join(after, Some(right.clone()))))
            } else {
                let (before, after) = split(Some(right.clone()), offset - left.bytes)
                    .map_err(|error| shifted(error, left.bytes))?;
                Ok((join(Some(left.clone()), before), after))
            }
        }
    }
}

pub(super) fn visit<E>(
    tree: &Tree,
    range: Range<u64>,
    base: u64,
    visitor: &mut impl FnMut(&Piece, Range<u64>, u64) -> Result<(), E>,
) -> Result<(), E> {
    let Some(node) = tree else {
        return Ok(());
    };
    if range.is_empty() {
        return Ok(());
    }
    match &node.kind {
        Kind::Leaf(piece) => visitor(piece, range, base),
        Kind::Branch(left, right) => {
            if range.start < left.bytes {
                visit(
                    &Some(left.clone()),
                    range.start..range.end.min(left.bytes),
                    base,
                    visitor,
                )?;
            }
            if range.end > left.bytes {
                visit(
                    &Some(right.clone()),
                    range.start.saturating_sub(left.bytes)..range.end - left.bytes,
                    base + left.bytes,
                    visitor,
                )?;
            }
            Ok(())
        }
    }
}

pub(super) fn bytes(tree: &Tree, range: Range<u64>) -> Result<Bytes, ReadError> {
    let mut parts = Vec::new();
    visit(tree, range.clone(), 0, &mut |piece, local, base| {
        parts.push(piece.bytes(local).map_err(|error| shifted(error, base))?);
        Ok(())
    })?;
    if parts.len() == 1 {
        return Ok(parts.pop().unwrap());
    }
    let mut bytes = BytesMut::with_capacity((range.end - range.start) as usize);
    for part in parts {
        bytes.extend_from_slice(&part);
    }
    Ok(bytes.freeze())
}

pub(super) fn line_at(tree: &Tree, offset: u64) -> Result<u64, ReadError> {
    let Some(node) = tree else {
        return Ok(0);
    };
    if offset == node.bytes {
        return Ok(node.newlines);
    }
    match &node.kind {
        Kind::Leaf(piece) => Ok(memchr_iter(b'\n', &piece.bytes(0..offset)?).count() as u64),
        Kind::Branch(left, right) => {
            if offset < left.bytes {
                line_at(&Some(left.clone()), offset)
            } else {
                line_at(&Some(right.clone()), offset - left.bytes)
                    .map(|lines| lines + left.newlines)
                    .map_err(|error| shifted(error, left.bytes))
            }
        }
    }
}

pub(super) fn line_start(tree: &Tree, line: u64) -> Result<Option<u64>, ReadError> {
    if line == 0 {
        return Ok(Some(0));
    }
    let Some(node) = tree else {
        return Ok(None);
    };
    if line > node.newlines {
        return Ok(None);
    }
    match &node.kind {
        Kind::Leaf(piece) => Ok(memchr_iter(b'\n', &piece.bytes(0..piece.len())?)
            .nth(line as usize - 1)
            .map(|offset| offset as u64 + 1)),
        Kind::Branch(left, right) => {
            if line <= left.newlines {
                line_start(&Some(left.clone()), line)
            } else {
                line_start(&Some(right.clone()), line - left.newlines)
                    .map(|offset| offset.map(|offset| offset + left.bytes))
                    .map_err(|error| shifted(error, left.bytes))
            }
        }
    }
}

pub(super) fn unicode_prefix(tree: &Tree, offset: u64) -> Result<UnicodeSummary, ReadError> {
    let Some(node) = tree else {
        return Ok(UnicodeSummary::default());
    };
    if offset == 0 {
        return Ok(UnicodeSummary::default());
    }
    if offset == node.bytes {
        return Ok(node.unicode);
    }
    match &node.kind {
        Kind::Leaf(piece) => Ok(UnicodeSummary::new(&piece.bytes(0..offset)?)),
        Kind::Branch(left, right) => {
            if offset < left.bytes {
                unicode_prefix(&Some(left.clone()), offset)
            } else {
                let prefix = unicode_prefix(&Some(right.clone()), offset - left.bytes)
                    .map_err(|error| shifted(error, left.bytes))?;
                Ok(left.unicode.join(left.bytes, prefix, offset - left.bytes))
            }
        }
    }
}

pub(super) fn unicode_offset(
    tree: &Tree,
    units: u64,
    utf16: bool,
    after: bool,
) -> Result<Option<u64>, ReadError> {
    let count = |summary: UnicodeSummary| {
        if utf16 {
            summary.utf16
        } else {
            summary.scalars
        }
    };
    let Some(node) = tree else {
        return Ok((units == 0).then_some(0));
    };
    if units > count(node.unicode) {
        return Ok(None);
    }
    // ASCII and isolated malformed bytes each occupy exactly one unit. The
    // summary alone answers these lookups, even when their bytes are uncached.
    if node.unicode.scalars == node.bytes {
        return Ok(Some(units));
    }
    if units == 0 {
        return Ok(Some(0));
    }
    if units == count(node.unicode) {
        return Ok(Some(node.bytes));
    }
    match &node.kind {
        Kind::Leaf(piece) => {
            let bytes = piece.bytes(0..piece.len())?;
            let mut offset = 0;
            let mut at = 0;
            while offset < bytes.len() {
                if units == at {
                    return Ok(Some(offset as u64));
                }
                let (ch, len) = crate::unicode_summary::scalar(&bytes[offset..]);
                let width = if utf16 { ch.len_utf16() as u64 } else { 1 };
                if units < at + width {
                    return Ok(Some((offset + if after { len } else { 0 }) as u64));
                }
                at += width;
                offset += len;
            }
            Ok(Some(offset as u64))
        }
        Kind::Branch(left, right) => {
            let left_units = count(left.unicode);
            let right_units = if let Some(crossing) =
                left.unicode
                    .crossing(left.bytes, right.unicode, right.bytes)
            {
                let before = left_units - crossing.left;
                let width = if utf16 { crossing.utf16 } else { 1 };
                if units < before {
                    return unicode_offset(&Some(left.clone()), units, utf16, after);
                }
                if units <= before + width {
                    return Ok(Some(
                        if units == before || (units < before + width && !after) {
                            left.bytes - crossing.left
                        } else {
                            left.bytes + crossing.right
                        },
                    ));
                }
                units - before - width + crossing.right
            } else {
                if units <= left_units {
                    return unicode_offset(&Some(left.clone()), units, utf16, after);
                }
                units - left_units
            };
            unicode_offset(&Some(right.clone()), right_units, utf16, after)
                .map(|offset| offset.map(|offset| offset + left.bytes))
                .map_err(|error| shifted(error, left.bytes))
        }
    }
}

#[cfg(test)]
pub(super) fn check(tree: &Tree) {
    if let Some(node) = tree {
        match &node.kind {
            Kind::Leaf(piece) => {
                assert!((1..=LEAF_BYTES as u64).contains(&piece.len()));
                assert_eq!(node.bytes, piece.len());
                assert_eq!(node.height, 1);
                assert_eq!(node.newlines, piece.newlines);
                if let Ok(bytes) = piece.bytes(0..piece.len()) {
                    assert_eq!(node.unicode, UnicodeSummary::new(&bytes));
                }
            }
            Kind::Branch(left, right) => {
                check(&Some(left.clone()));
                check(&Some(right.clone()));
                assert!(left.height.abs_diff(right.height) <= 1);
                assert_eq!(node.height, 1 + left.height.max(right.height));
                assert_eq!(node.bytes, left.bytes + right.bytes);
                assert_eq!(node.newlines, left.newlines + right.newlines);
                assert_eq!(
                    node.unicode,
                    left.unicode.join(left.bytes, right.unicode, right.bytes)
                );
            }
        }
    }
}
