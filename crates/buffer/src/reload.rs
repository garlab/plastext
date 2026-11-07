//! Off-thread position mapping for external reloads.
use crate::{Anchor, Bias, ByteSource, ReadError, TextSnapshot};
use std::{collections::BTreeMap, io, sync::atomic::AtomicBool};

pub(crate) enum Mapping {
    Diff(Vec<plastext_diff::Change>),
    Lines(BTreeMap<u64, u64>),
}

impl Mapping {
    pub fn prepare(
        old: &TextSnapshot,
        new: &TextSnapshot,
        positions: &[u64],
        cancel: &AtomicBool,
    ) -> io::Result<Self> {
        if old.len().max(new.len()) <= 8 * 1024 * 1024 {
            let mut a = Vec::new();
            let mut b = Vec::new();
            old.read_uncached(0..old.len(), &mut a)?;
            new.read_uncached(0..new.len(), &mut b)?;
            crate::edit::check_cancel(cancel)?;
            let mut changes = plastext_diff::changes(&a, &b);
            for change in &mut changes {
                while !change.before.is_empty()
                    && !change.after.is_empty()
                    && a[change.before.start] == b[change.after.start]
                {
                    change.before.start += 1;
                    change.after.start += 1;
                }
                while !change.before.is_empty()
                    && !change.after.is_empty()
                    && a[change.before.end - 1] == b[change.after.end - 1]
                {
                    change.before.end -= 1;
                    change.after.end -= 1;
                }
            }
            changes.retain(|c| !c.before.is_empty() || !c.after.is_empty());
            return Ok(Self::Diff(changes));
        }
        let mut mapped = BTreeMap::new();
        for &offset in positions {
            crate::edit::check_cancel(cancel)?;
            let offset = offset.min(old.len());
            let line = loading(old, || old.line_at(offset))?;
            let start = loading(old, || old.line_start(line))?.unwrap_or(old.len());
            let line = line.min(new.line_count() - 1);
            let new_start = loading(new, || new.line_start(line))?.unwrap_or(new.len());
            let end = loading(new, || new.line_start(line + 1))?.unwrap_or(new.len());
            mapped.insert(offset, (new_start + offset - start).min(end));
        }
        Ok(Self::Lines(mapped))
    }
    pub fn map(&self, anchor: Anchor) -> Option<u64> {
        match self {
            Self::Lines(positions) => positions.get(&anchor.offset).copied(),
            Self::Diff(changes) => {
                let offset = anchor.offset as usize;
                let index = changes.partition_point(|c| c.before.end < offset);
                if let Some(c) = changes.get(index)
                    && offset >= c.before.start
                {
                    return Some(
                        if c.before.is_empty()
                            || offset == c.before.end
                            || anchor.bias == Bias::After
                        {
                            c.after.end as u64
                        } else {
                            c.after.start as u64
                        },
                    );
                }
                let previous = index
                    .checked_sub(1)
                    .map_or((0, 0), |i| (changes[i].before.end, changes[i].after.end));
                Some((previous.1 + offset - previous.0) as u64)
            }
        }
    }
}

fn loading<T>(
    text: &TextSnapshot,
    mut action: impl FnMut() -> Result<T, ReadError>,
) -> io::Result<T> {
    loop {
        match action() {
            Ok(value) => return Ok(value),
            Err(ReadError::Missing(range)) => text.load(range)?,
            Err(error) => return Err(io::Error::other(format!("{error:?}"))),
        }
    }
}
