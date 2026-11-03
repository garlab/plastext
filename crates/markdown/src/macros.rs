//! A bounded, source-ordered history of formulas that define TeX macros.
//! Store it once, not once per formula or parsed page. Rendering replays this
//! small history with KaTeX, so scrolling backwards cannot change definitions.
use crate::{Block, BlockKind};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

const MAX_BYTES: usize = 128 * 1024;
const MAX_DEFINITIONS: usize = 256;

#[derive(Clone, Debug)]
struct Definition {
    end: usize,
    tex: String,
    display: bool,
}

#[derive(Default, Debug)]
pub struct MacroHistory {
    definitions: Arc<Vec<Definition>>,
    bytes: usize,
    overflow: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct MacroContext {
    definitions: Arc<Vec<Definition>>,
    count: usize,
    overflow: bool,
}

impl PartialEq for MacroContext {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.overflow == other.overflow
            && (self.count == 0 || Arc::ptr_eq(&self.definitions, &other.definitions))
    }
}
impl Eq for MacroContext {}
impl Hash for MacroContext {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.count.hash(state);
        self.overflow.hash(state);
        if self.count != 0 {
            (Arc::as_ptr(&self.definitions) as usize).hash(state);
        }
    }
}
impl MacroContext {
    pub fn is_empty(&self) -> bool {
        self.count == 0 && !self.overflow
    }
    pub fn sources(&self) -> Result<impl Iterator<Item = (&str, bool)>, &'static str> {
        if self.overflow {
            return Err("Document math macro limit exceeded; showing TeX source.");
        }
        Ok(self.definitions[..self.count]
            .iter()
            .map(|d| (d.tex.as_str(), d.display)))
    }
}

impl MacroHistory {
    pub fn at(&self, source: usize) -> MacroContext {
        MacroContext {
            count: self.definitions.partition_point(|d| d.end <= source),
            definitions: self.definitions.clone(),
            overflow: self.overflow.is_some_and(|at| source >= at),
        }
    }
    pub(crate) fn collect(&mut self, block: &Block) {
        if matches!(block.kind, BlockKind::Math) {
            self.push(block.source.end, &block.text, true);
        } else if !matches!(block.kind, BlockKind::Code { .. } | BlockKind::Html) {
            for span in block.spans.iter().filter(|s| s.style.math) {
                self.push(span.source.end, &block.text[span.range.clone()], false);
            }
        }
    }
    fn push(&mut self, end: usize, tex: &str, display: bool) {
        if self.overflow.is_some() || !defines_macros(tex) {
            return;
        }
        if self.bytes + tex.len() > MAX_BYTES || self.definitions.len() >= MAX_DEFINITIONS {
            self.overflow = Some(end);
            return;
        }
        self.bytes += tex.len();
        // Contexts used while indexing are short-lived. Once published the
        // history is immutable, and all visible formulas share this allocation.
        Arc::make_mut(&mut self.definitions).push(Definition {
            end,
            tex: tex.to_owned(),
            display,
        });
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes + self.definitions.capacity() * size_of::<Definition>()
    }
}

fn defines_macros(tex: &str) -> bool {
    tex.split('\\').skip(1).any(|command| {
        let end = command
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(command.len());
        matches!(
            &command[..end],
            "newcommand"
                | "renewcommand"
                | "providecommand"
                | "def"
                | "gdef"
                | "edef"
                | "xdef"
                | "let"
                | "futurelet"
                | "global"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collects_math_only_in_source_order_including_headings() {
        let text = "```tex\n\\def\\fake{x}\n```\n\n$\\def\\a{1}$ and $\\a$\n\n# $\\newcommand{\\b}{2}$\n\n$$\\b$$";
        let parsed = crate::parse(text);
        let mut history = MacroHistory::default();
        for b in &parsed.blocks {
            history.collect(b);
        }
        assert_eq!(
            history
                .at(text.find("and").unwrap())
                .sources()
                .unwrap()
                .count(),
            1
        );
        assert_eq!(history.at(text.len()).sources().unwrap().count(), 2);
        assert_eq!(history.at(0).sources().unwrap().count(), 0);
    }
    #[test]
    fn history_is_bounded_and_context_identity_is_document_scoped() {
        let mut history = MacroHistory::default();
        for i in 0..MAX_DEFINITIONS + 1 {
            history.push(i + 1, r"\def\x{1}", true);
        }
        assert!(history.at(MAX_DEFINITIONS + 1).sources().is_err());
        assert!(history.at(MAX_DEFINITIONS).sources().is_ok());
        let mut other = MacroHistory::default();
        other.push(1, r"\def\x{2}", true);
        assert_ne!(history.at(1), other.at(1));
    }
}
