//! Diff computation, word diffs and the review-page model (spec §5.7).

use std::{collections::HashMap, ops::Range};

/// A replacement in original byte coordinates. Changes are ordered and disjoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub before: Range<usize>,
    pub after: Range<usize>,
}

/// Bounded patience diff: unique lines anchor unchanged content. Repeated-line
/// regions without anchors become one replacement. No quadratic edit matrix.
pub fn changes(before: &[u8], after: &[u8]) -> Vec<Change> {
    if before == after {
        return Vec::new();
    }
    if before.iter().filter(|b| **b == b'\n').take(100_001).count() > 100_000
        || after.iter().filter(|b| **b == b'\n').take(100_001).count() > 100_000
    {
        return vec![Change {
            before: 0..before.len(),
            after: 0..after.len(),
        }];
    }
    let a = lines(before);
    let b = lines(after);
    let mut output = Vec::new();
    let mut pending = vec![(0..a.len(), 0..b.len())];
    let mut budget = 1_000_000usize;
    while let Some((mut x, mut y)) = pending.pop() {
        while !x.is_empty() && !y.is_empty() && a[x.start].1 == b[y.start].1 {
            x.start += 1;
            y.start += 1;
        }
        while !x.is_empty() && !y.is_empty() && a[x.end - 1].1 == b[y.end - 1].1 {
            x.end -= 1;
            y.end -= 1;
        }
        if x.is_empty() && y.is_empty() {
            continue;
        }
        let mut anchors = Vec::new();
        if x.len() + y.len() <= budget && !x.is_empty() && !y.is_empty() {
            budget -= x.len() + y.len();
            let mut unique = HashMap::new();
            for i in x.clone() {
                unique
                    .entry(a[i].1)
                    .and_modify(|v| *v = None)
                    .or_insert(Some(i));
            }
            let mut matches = HashMap::new();
            for j in y.clone() {
                if let Some(Some(i)) = unique.get(b[j].1) {
                    matches
                        .entry(*i)
                        .and_modify(|v| *v = None)
                        .or_insert(Some(j));
                }
            }
            let pairs: Vec<_> = x
                .clone()
                .filter_map(|i| Some((i, matches.get(&i).copied()??)))
                .collect();
            let mut tails: Vec<usize> = Vec::new();
            let mut previous = vec![None; pairs.len()];
            for (index, &(_, j)) in pairs.iter().enumerate() {
                let slot = tails.partition_point(|&tail| pairs[tail].1 < j);
                previous[index] = slot.checked_sub(1).map(|s| tails[s]);
                if slot == tails.len() {
                    tails.push(index);
                } else {
                    tails[slot] = index;
                }
            }
            let mut cursor = tails.last().copied();
            while let Some(index) = cursor {
                anchors.push(pairs[index]);
                cursor = previous[index];
            }
            anchors.reverse();
        }
        if anchors.is_empty() {
            let start =
                |lines: &[(usize, &[u8])], index: usize, len| lines.get(index).map_or(len, |l| l.0);
            output.push(Change {
                before: start(&a, x.start, before.len())..start(&a, x.end, before.len()),
                after: start(&b, y.start, after.len())..start(&b, y.end, after.len()),
            });
        } else {
            let mut i = x.start;
            let mut j = y.start;
            for (ai, bj) in anchors {
                pending.push((i..ai, j..bj));
                i = ai + 1;
                j = bj + 1;
            }
            pending.push((i..x.end, j..y.end));
        }
    }
    output.sort_by_key(|change| (change.before.start, change.after.start));
    output
}

fn lines(bytes: &[u8]) -> Vec<(usize, &[u8])> {
    let mut offset = 0;
    bytes
        .split_inclusive(|b| *b == b'\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, line)
        })
        .collect()
}

/// A unified diff for bounded UTF-8 snapshots. Callers enforce input/output limits.
pub fn unified(before: &[u8], after: &[u8], changes: &[Change]) -> String {
    use std::fmt::Write;
    let mut output = String::from("--- Disk\n+++ Your changes\n");
    let mut previous = (0, 0);
    let mut line_numbers = (0, 0);
    for change in changes {
        let old = &before[change.before.clone()];
        let new = &after[change.after.clone()];
        let old_count = lines(old).len();
        let new_count = lines(new).len();
        line_numbers.0 += before[previous.0..change.before.start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        line_numbers.1 += after[previous.1..change.after.start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        let old_line = line_numbers.0 + usize::from(old_count != 0);
        let new_line = line_numbers.1 + usize::from(new_count != 0);
        writeln!(
            output,
            "@@ -{old_line},{old_count} +{new_line},{new_count} @@"
        )
        .unwrap();
        for (prefix, bytes) in [('-', old), ('+', new)] {
            for (_, line) in lines(bytes) {
                output.push(prefix);
                output.push_str(&String::from_utf8_lossy(line));
                if !line.ends_with(b"\n") {
                    output.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
        previous = (change.before.end, change.after.end);
        line_numbers.0 += old_count;
        line_numbers.1 += new_count;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disjoint_changes_preserve_middle_lines_and_reconstruct_the_result() {
        for (a, b) in [
            ("a\nb\nc\nd\n", "prefix\na\nb!\nc\nd\ntail"),
            ("x\nx\nx\n", "x\ny\nx\n"),
            ("", "new"),
            ("old", ""),
            ("same", "same"),
        ] {
            let changes = changes(a.as_bytes(), b.as_bytes());
            let mut result = a.as_bytes().to_vec();
            for change in changes.iter().rev() {
                result.splice(
                    change.before.clone(),
                    b.as_bytes()[change.after.clone()].iter().copied(),
                );
            }
            assert_eq!(result, b.as_bytes());
        }
        assert_eq!(changes(b"a\nb\nc\nd\n", b"a\nB\nc\nD\n").len(), 2);
    }
}
