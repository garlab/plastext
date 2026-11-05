//! Persistent AVL tree of byte ranges. Leaves never exceed 64 KiB, so an
//! edit or line lookup only scans a bounded leaf after its logarithmic descent.
use std::{ops::Range, sync::Arc};

use crate::line_summary::LineSummary;
use bytes::{Bytes, BytesMut};

use crate::unicode_summary::UnicodeSummary;
use crate::{ByteSource, FileSource, ReadError};

pub(super) const LEAF_BYTES: usize = 64 * 1024;
pub(super) type Tree = Option<Arc<Node>>;
const NODE_BYTES: u64 = (size_of::<Node>() + 2 * size_of::<usize>()) as u64;

#[derive(Clone)]
enum Source {
    Original(Arc<FileSource>),
    Decoded(Arc<crate::encoding::DecodedSource>),
    Added(Bytes),
}

#[derive(Clone)]
pub(super) struct Piece {
    source: Source,
    range: Range<u64>,
}

pub(super) struct Node {
    pub bytes: u64,
    pub lines: LineSummary,
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
    fn leaf(piece: Piece, lines: LineSummary, unicode: UnicodeSummary) -> Arc<Self> {
        let retained = NODE_BYTES
            + match &piece.source {
                Source::Original(_) | Source::Decoded(_) => 0,
                Source::Added(bytes) => bytes.len() as u64 + 32,
            };
        Arc::new(Self {
            bytes: piece.range.end - piece.range.start,
            lines,
            unicode,
            height: 1,
            retained,
            kind: Kind::Leaf(piece),
        })
    }

    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            bytes: left.bytes + right.bytes,
            lines: left.lines.join(right.lines),
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
            Source::Decoded(file) => file.text.bytes(source).map_err(|error| match error {
                ReadError::Missing(_) => ReadError::Missing(local),
                other => other,
            }),
            Source::Added(bytes) => Ok(bytes.slice(source.start as usize..source.end as usize)),
        }
    }

    pub fn missing(&self, local: Range<u64>) -> Option<Range<u64>> {
        let source = self.range.start + local.start..self.range.start + local.end;
        let missing = match &self.source {
            Source::Original(file) => file.missing(source),
            Source::Decoded(file) => file.text.missing(source),
            Source::Added(_) => None,
        };
        missing.map(|_| local)
    }

    pub fn load(&self, local: Range<u64>) -> std::io::Result<()> {
        let file = match &self.source {
            Source::Original(file) => Some(file),
            Source::Decoded(file) => Some(&file.text),
            Source::Added(_) => None,
        };
        if let Some(file) = file {
            file.load(self.range.start + local.start..self.range.start + local.end)?;
        }
        Ok(())
    }

    pub fn serialize(
        &self,
        local: Range<u64>,
        buffer: &mut [u8],
        output: &mut crate::serialize::Serializer<impl std::io::Write>,
    ) -> std::io::Result<()> {
        if let Source::Decoded(source) = &self.source {
            output.decoded(
                source,
                self.range.start + local.start..self.range.start + local.end,
            )
        } else {
            let bytes = &mut buffer[..(local.end - local.start) as usize];
            self.read_uncached(local, bytes)?;
            output.utf8(bytes)
        }
    }

    pub fn read_uncached(&self, local: Range<u64>, buffer: &mut [u8]) -> std::io::Result<()> {
        let range = self.range.start + local.start..self.range.start + local.end;
        match &self.source {
            Source::Original(file) => file.read_uncached(range, buffer),
            Source::Decoded(file) => file.text.read_uncached(range, buffer),
            Source::Added(bytes) => {
                buffer.copy_from_slice(&bytes[range.start as usize..range.end as usize]);
                Ok(())
            }
        }
    }
}

pub(super) fn original(
    source: Arc<FileSource>,
    leaves: &[crate::encoding::LeafSummary],
    offset: u64,
) -> Tree {
    from_source(Source::Original(source), leaves, offset)
}

pub(super) fn decoded(
    source: Arc<crate::encoding::DecodedSource>,
    leaves: &[crate::encoding::LeafSummary],
) -> Tree {
    from_source(Source::Decoded(source), leaves, 0)
}

fn from_source(source: Source, leaves: &[crate::encoding::LeafSummary], mut offset: u64) -> Tree {
    let pieces = leaves
        .iter()
        .map(|&(len, lines, unicode)| {
            let range = offset..offset + len;
            offset += len;
            (
                Piece {
                    source: source.clone(),
                    range,
                },
                lines,
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
                },
                LineSummary::new(chunk),
                UnicodeSummary::new(chunk),
            )
        })
        .collect::<Vec<_>>();
    build(&pieces)
}

fn build(pieces: &[(Piece, LineSummary, UnicodeSummary)]) -> Tree {
    match pieces {
        [] => None,
        [(piece, lines, unicode)] => Some(Node::leaf(piece.clone(), *lines, *unicode)),
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
            let (left_lines, right_lines) = node.lines.split(&bytes, at);
            let mut left = piece.clone();
            left.range.end = piece.range.start + offset;
            let mut right = piece.clone();
            right.range.start = left.range.end;
            Ok((
                Some(Node::leaf(left, left_lines, left_unicode)),
                Some(Node::leaf(right, right_lines, right_unicode)),
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

fn line_prefix(tree: &Tree, offset: u64) -> Result<LineSummary, ReadError> {
    let Some(node) = tree else {
        return Ok(LineSummary::new(&[]));
    };
    if offset == 0 {
        return Ok(LineSummary::new(&[]));
    }
    if offset == node.bytes {
        return Ok(node.lines);
    }
    match &node.kind {
        Kind::Leaf(piece) => Ok(LineSummary::new(&piece.bytes(0..offset)?)),
        Kind::Branch(left, right) => {
            if offset < left.bytes {
                line_prefix(&Some(left.clone()), offset)
            } else {
                let suffix = line_prefix(&Some(right.clone()), offset - left.bytes)
                    .map_err(|error| shifted(error, left.bytes))?;
                Ok(left.lines.join(suffix))
            }
        }
    }
}

pub(super) fn line_at(tree: &Tree, offset: u64) -> Result<u64, ReadError> {
    let prefix = line_prefix(tree, offset)?;
    // The caret between CR and LF still belongs to the preceding line.
    let crossing =
        prefix.ends_cr && offset < len(tree) && bytes(tree, offset..offset + 1)?[0] == b'\n';
    Ok(prefix.counts.count() - u64::from(crossing))
}

pub(super) fn line_start(tree: &Tree, line: u64) -> Result<Option<u64>, ReadError> {
    if line == 0 {
        return Ok(Some(0));
    }
    let Some(node) = tree else {
        return Ok(None);
    };
    if line > node.lines.counts.count() {
        return Ok(None);
    }
    match &node.kind {
        Kind::Leaf(piece) => {
            let bytes = piece.bytes(0..piece.len())?;
            let mut remaining = line;
            for at in memchr::memchr2_iter(b'\r', b'\n', &bytes) {
                if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    continue;
                }
                remaining -= 1;
                if remaining == 0 {
                    return Ok(Some(at as u64 + 1));
                }
            }
            unreachable!("line summary matches leaf");
        }
        Kind::Branch(left, right) => {
            let count = left.lines.counts.count();
            let crossing = left.lines.ends_cr && right.lines.starts_lf;
            if crossing && line == count {
                Ok(Some(left.bytes + 1))
            } else if line <= count {
                line_start(&Some(left.clone()), line)
            } else {
                line_start(&Some(right.clone()), line - count + u64::from(crossing))
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
                if let Ok(bytes) = piece.bytes(0..piece.len()) {
                    assert_eq!(node.lines, LineSummary::new(&bytes));
                    assert_eq!(node.unicode, UnicodeSummary::new(&bytes));
                }
            }
            Kind::Branch(left, right) => {
                check(&Some(left.clone()));
                check(&Some(right.clone()));
                assert!(left.height.abs_diff(right.height) <= 1);
                assert_eq!(node.height, 1 + left.height.max(right.height));
                assert_eq!(node.bytes, left.bytes + right.bytes);
                assert_eq!(node.lines, left.lines.join(right.lines));
                assert_eq!(
                    node.unicode,
                    left.unicode.join(left.bytes, right.unicode, right.bytes)
                );
            }
        }
    }
}
