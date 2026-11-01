//! Math, rendered off the UI thread and cached like images.
//!
//! Display formulas are blocks of their own, with any tag or equation
//! numbers against the right edge. An inline formula takes the place of its
//! TeX in the text as a run of transparent `l`s about as wide as the
//! formula: the text wraps around it like a word, and the formula is painted
//! over it once the text is laid out.
use super::*;
use gpui::{Bounds, Corners, TextLayout, canvas, point, size};

/// Formulas rendering at once, and kept rendered.
const MAX_PENDING: usize = 4;
const MAX_CACHED: usize = 256;

/// Stands in for an inline formula: a word character, so lines never break
/// inside the run, with no kerning or ligatures against itself.
pub(super) const SLOT_CHAR: char = 'l';

/// Everything a rendered formula depends on.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct FormulaKey {
    tex: String,
    display: bool,
    /// The first automatic equation number, or 0 if it numbers none.
    first_number: u32,
    color: [u8; 4],
    /// Device pixels per logical pixel, in thousandths.
    scale: u32,
    /// Surrounding text size in logical pixels, in thousandths (including zoom).
    font_size: u32,
}

/// A rendered piece of a formula, in logical pixels.
#[derive(Clone)]
pub(super) struct Image {
    image: Arc<gpui::RenderImage>,
    width: f32,
    height: f32,
    /// The baseline, from the top.
    baseline: f32,
}

pub(super) struct Formula {
    body: Image,
    /// The tag or equation numbers of display math.
    tag: Option<Image>,
}

#[derive(Default)]
pub(super) struct Formulas {
    cache: HashMap<FormulaKey, Option<Result<Formula, String>>>,
    order: VecDeque<FormulaKey>,
    /// The list rows showing each formula, to re-measure when it arrives.
    rows: HashMap<FormulaKey, Vec<usize>>,
    /// Measured for the current window by [`MarkdownView::prepare_math`].
    inline: HashMap<u32, InlineMetrics>,
    /// The first equation number of each display formula that numbers its
    /// equations, by block. `None` until the document has finished parsing.
    numbering: Option<Arc<HashMap<usize, u32>>>,
}

impl Formulas {
    pub(super) fn zoom_changed(&mut self) {
        self.inline.clear();
        self.rows.clear();
    }

    /// Forgets what depends on the document's blocks, when they change.
    pub(super) fn clear_rows(&mut self) {
        self.rows.clear();
        self.numbering = None;
    }
}

/// What inline formulas need from the window, in logical pixels.
#[derive(Clone, Copy)]
struct InlineMetrics {
    /// The advance of [`SLOT_CHAR`] in the prose font.
    slot_char: f32,
    ascent: f32,
    descent: f32,
    line_height: f32,
    scale: f32,
}

/// An inline formula in laid-out text.
pub(super) struct Slot {
    /// Where its placeholder starts in the displayed text.
    start: usize,
    /// Byte offset of the hard line, for GPUI's per-line glyph indices.
    line_start: usize,
    /// How many [`SLOT_CHAR`]s it has.
    count: usize,
    /// The placeholder's width.
    width: f32,
    formula: Image,
}

fn channels(color: gpui::Rgba) -> [u8; 4] {
    [color.r, color.g, color.b, color.a].map(|value| (value.clamp(0., 1.) * 255.).round() as u8)
}

fn scale_key(scale: f32) -> u32 {
    (scale * 1000.).round() as u32
}

/// The color of an inline formula: a link's inside links, like its text.
pub(super) fn inline_color(style: &plastext_markdown::InlineStyle, theme: Theme) -> gpui::Rgba {
    if style.link.is_some() {
        theme.syntax(Highlight::Link)
    } else {
        theme.text
    }
}

impl MarkdownView {
    /// Measures what inline formulas need and requests the ones in
    /// `block`. Call before laying out the block's text.
    pub(super) fn prepare_math(
        &mut self,
        block: &Block,
        ix: usize,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !inline_math_shown(block) {
            return;
        }
        let scale = window.scale_factor();
        let font_size = text_size(&block.kind) * self.zoom;
        let size_key = scale_key(font_size);
        // The prose font only changes on restart.
        if !self
            .formulas
            .inline
            .get(&size_key)
            .is_some_and(|metrics| metrics.scale == scale)
        {
            let font = text_run(1, false, false, false, theme.text, self).font;
            let text_system = window.text_system();
            let font_id = text_system.resolve_font(&font);
            let size = px(font_size);
            if let Some(metrics) =
                text_system
                    .advance(font_id, size, SLOT_CHAR)
                    .ok()
                    .map(|slot_char| InlineMetrics {
                        slot_char: f32::from(slot_char.width),
                        ascent: f32::from(text_system.ascent(font_id, size)),
                        // Font metrics give the descent as negative.
                        descent: f32::from(text_system.descent(font_id, size)).abs(),
                        line_height: font_size * LINE_HEIGHT,
                        scale,
                    })
            {
                self.formulas.inline.insert(size_key, metrics);
            }
        }
        if !self.formulas.inline.contains_key(&size_key) {
            return;
        }
        for span in block.spans.iter().filter(|span| span.style.math) {
            let key = FormulaKey {
                tex: block.text[span.range.clone()].to_owned(),
                display: false,
                first_number: 0,
                color: channels(inline_color(&span.style, theme)),
                scale: scale_key(scale),
                font_size: size_key,
            };
            self.request_formula(&key, ix, scale, cx);
        }
    }

    /// The placeholder for an inline formula: how many [`SLOT_CHAR`]s, and
    /// how wide they are. `None` until the formula has rendered.
    pub(super) fn inline_slot(
        &self,
        block: &Block,
        tex: &str,
        color: gpui::Rgba,
    ) -> Option<(usize, f32, Image)> {
        let size_key = scale_key(text_size(&block.kind) * self.zoom);
        let metrics = self.formulas.inline.get(&size_key)?;
        let key = FormulaKey {
            tex: tex.to_owned(),
            display: false,
            first_number: 0,
            color: channels(color),
            scale: scale_key(metrics.scale),
            font_size: size_key,
        };
        let formula = &self.formulas.cache.get(&key)?.as_ref()?.as_ref().ok()?.body;
        let count = ((formula.width / metrics.slot_char).round() as usize).max(1);
        Some((count, count as f32 * metrics.slot_char, formula.clone()))
    }

    /// `text` with its inline formulas painted over their placeholders,
    /// with lines tall enough for the tallest one.
    pub(super) fn with_inline_math(
        &self,
        block: &Block,
        text: InteractiveText,
        layout: TextLayout,
        slots: Vec<Slot>,
    ) -> AnyElement {
        let size_key = scale_key(text_size(&block.kind) * self.zoom);
        let Some(&metrics) = self
            .formulas
            .inline
            .get(&size_key)
            .filter(|_| !slots.is_empty())
        else {
            return text.into_any_element();
        };
        // GPUI centers glyphs in the line: the baseline sits
        // `(line - ascent - descent) / 2 + ascent` below the line's top.
        let (ascent, descent) = (metrics.ascent, metrics.descent);
        let line_height = slots.iter().fold(metrics.line_height, |line, slot| {
            let above = slot.formula.baseline;
            let below = slot.formula.height - above;
            line.max(2. * above - ascent + descent)
                .max(2. * below + ascent - descent)
        });
        div()
            .relative()
            .line_height(px(line_height.ceil()))
            .child(text)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        for slot in &slots {
                            paint_slot(&layout, slot, metrics.slot_char, window);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }

    /// Works out where each display formula's equation numbers start, so
    /// they count up through the document as on a web page.
    pub(super) fn number_equations(&mut self, cx: &mut Context<Self>) {
        self.formulas.numbering = Some(Arc::new(self.document.equation_starts.clone()));
        cx.notify();
    }

    /// A display formula, or its TeX while it renders or when it can't.
    pub(super) fn display_math(
        &mut self,
        block: &Block,
        ix: usize,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if block.text.trim().is_empty() {
            return div().into_any_element();
        }
        let scale = window.scale_factor();
        // Numbered formulas wait until their first number is known.
        let first_number = if plastext_math::may_number(&block.text) {
            self.formulas
                .numbering
                .as_ref()
                .map(|starts| starts.get(&ix).copied().unwrap_or(1))
        } else {
            Some(0)
        };
        let key = first_number.map(|first_number| FormulaKey {
            tex: block.text.to_string(),
            display: true,
            first_number,
            color: channels(theme.text),
            scale: scale_key(scale),
            font_size: scale_key(BODY_SIZE * self.zoom),
        });
        if let Some(key) = &key {
            self.request_formula(key, ix, scale, cx);
        }
        // Without the newlines that usually surround `$$` math.
        let start = block.text.len() - block.text.trim_start().len();
        let end = block.text.trim_end().len().max(start);
        let source = || {
            div().p_3().bg(theme.surface).child(self.rich_text(
                block,
                start..end,
                ix,
                true,
                theme,
                cx,
            ))
        };
        let rendered = key
            .as_ref()
            .and_then(|key| self.formulas.cache.get(key))
            .and_then(Option::as_ref);
        match rendered {
            Some(Ok(formula)) => display_formula(formula, ix),
            Some(Err(error)) => source()
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(error.clone()),
                )
                .into_any_element(),
            None => source().into_any_element(),
        }
    }

    fn request_formula(&mut self, key: &FormulaKey, ix: usize, scale: f32, cx: &mut Context<Self>) {
        let rows = self.formulas.rows.entry(key.clone()).or_default();
        if !rows.contains(&ix) && rows.len() < 128 {
            rows.push(ix);
        }
        let pending = self
            .formulas
            .cache
            .values()
            .filter(|entry| entry.is_none())
            .count();
        if self.formulas.cache.contains_key(key) || pending >= MAX_PENDING {
            return;
        }
        self.formulas.cache.insert(key.clone(), None);
        self.formulas.order.push_back(key.clone());
        let rendering = cx.background_executor().spawn({
            let key = key.clone();
            let body_size = key.font_size as f32 / 1000.;
            async move {
                let px_per_em = body_size * plastext_math::SCALE * scale;
                let rendered = plastext_math::render(
                    &key.tex,
                    key.display,
                    key.first_number.max(1),
                    px_per_em,
                    key.color,
                )?;
                Ok(Formula {
                    body: image(rendered.body, scale)?,
                    tag: rendered.tag.map(|tag| image(tag, scale)).transpose()?,
                })
            }
        });
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let formula = rendering.await;
            this.update(cx, |this, cx| {
                let formulas = &mut this.formulas;
                formulas.cache.insert(key.clone(), Some(formula));
                while formulas.order.len() > MAX_CACHED {
                    if let Some(old) = formulas.order.pop_front() {
                        formulas.cache.remove(&old);
                        formulas.rows.remove(&old);
                    }
                }
                // Rows change height once the formula replaces its TeX.
                if let Some(rows) = this.formulas.rows.get(&key) {
                    let anchor = this.list.logical_scroll_top();
                    for &row in rows {
                        if row < this.document.blocks.len() {
                            let visible = this.visible.row(row);
                            this.list.splice(visible..visible + 1, 1);
                        }
                    }
                    this.list.scroll_to(anchor);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A GPUI image of `raster`, rendered at `scale` device pixels per point.
fn image(raster: plastext_math::Raster, scale: f32) -> Result<Image, String> {
    let mut bgra = raster.rgba;
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(raster.width, raster.height, bgra)
        .ok_or("The formula's image has the wrong size.")?;
    Ok(Image {
        image: Arc::new(gpui::RenderImage::new([image::Frame::new(buffer)])),
        width: raster.width as f32 / scale,
        height: raster.height as f32 / scale,
        baseline: raster.baseline / scale,
    })
}

/// A display formula centered in the column, with its tag against the right
/// edge on the same baseline, as KaTeX lays them out. Too wide a formula
/// scrolls sideways.
fn display_formula(formula: &Formula, ix: usize) -> AnyElement {
    let Formula { body, tag } = formula;
    let above = body
        .baseline
        .max(tag.as_ref().map_or(0., |tag| tag.baseline));
    let below =
        (body.height - body.baseline).max(tag.as_ref().map_or(0., |tag| tag.height - tag.baseline));
    div()
        .id(("md-math", ix))
        .w_full()
        .overflow_x_scroll()
        .child(
            div()
                .relative()
                .min_w_full()
                .h(px(above + below))
                .flex()
                .justify_center()
                .child(
                    gpui::img(body.image.clone())
                        .flex_none()
                        .mt(px(above - body.baseline))
                        .w(px(body.width))
                        .h(px(body.height)),
                )
                .children(tag.as_ref().map(|tag| {
                    gpui::img(tag.image.clone())
                        .absolute()
                        .right_0()
                        .top(px(above - tag.baseline))
                        .w(px(tag.width))
                        .h(px(tag.height))
                })),
        )
        .into_any_element()
}

/// Display math already renders as a whole formula; its fallback stays source.
pub(super) fn inline_math_shown(block: &Block) -> bool {
    !matches!(block.kind, BlockKind::Math)
}

/// Shared by heading text, inline placeholders and formula rasterization.
pub(super) fn text_size(kind: &BlockKind) -> f32 {
    match kind {
        BlockKind::Heading { level: 1, .. } => 30.,
        BlockKind::Heading { level: 2, .. } => 24.,
        BlockKind::Heading { level: 3, .. } => 20.,
        BlockKind::Heading { .. } => 17.,
        _ => BODY_SIZE,
    }
}

impl Slot {
    pub(super) fn new(start: usize, count: usize, width: f32, formula: Image) -> Self {
        Self {
            start,
            line_start: 0,
            count,
            width,
            formula,
        }
    }

    /// The placeholder's bytes in the displayed text.
    pub(super) fn range(&self) -> std::ops::Range<usize> {
        self.start..self.start + self.count * SLOT_CHAR.len_utf8()
    }

    pub(super) fn set_line_starts(slots: &mut [Self], text: &str) {
        // One forward scan, even for paragraphs dense with inline formulas.
        let mut newlines = text.match_indices('\n').map(|(index, _)| index).peekable();
        let mut line_start = 0;
        for slot in slots {
            while newlines.peek().is_some_and(|index| *index < slot.start) {
                line_start = newlines.next().unwrap() + 1;
            }
            slot.line_start = line_start;
        }
    }
}

fn paint_slot(layout: &TextLayout, slot: &Slot, slot_char: f32, window: &mut Window) {
    // An index at a wrap is reported at the end of the previous row, so
    // locate the placeholder by its first character's end instead.
    let Some(after_first) = layout.position_for_index(slot.start + SLOT_CHAR.len_utf8()) else {
        return;
    };
    let Some(line) = layout.line_layout_for_index(slot.start + SLOT_CHAR.len_utf8()) else {
        return;
    };
    let line_layout = &line.unwrapped_layout;
    let line_height = layout.line_height();
    let line_top = after_first.y;
    let baseline = line_top
        + (line_height - line_layout.ascent - line_layout.descent) / 2.
        + line_layout.ascent;
    // GPUI's index positions omit text-align's paint-time offset. Apply the
    // same offset to formulas, using this wrapped row's actual glyph width.
    let alignment = window.text_style().text_align;
    let offset = if alignment == gpui::TextAlign::Left {
        px(0.)
    } else {
        let index = slot.start + SLOT_CHAR.len_utf8() - slot.line_start;
        let glyph_at = |boundary: &gpui::WrapBoundary| {
            &line_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
        };
        let row = line
            .wrap_boundaries
            .partition_point(|boundary| glyph_at(boundary).index < index);
        let start = row.checked_sub(1).map_or(px(0.), |row| {
            glyph_at(&line.wrap_boundaries[row]).position.x
        });
        let end = line
            .wrap_boundaries
            .get(row)
            .map_or(line_layout.width, |boundary| glyph_at(boundary).position.x);
        let spare = layout.bounds().size.width - (end - start);
        if alignment == gpui::TextAlign::Center {
            spare / 2.
        } else {
            spare
        }
    };
    let left = after_first.x + offset - px(slot_char) + px((slot.width - slot.formula.width) / 2.);
    let bounds = Bounds::new(
        point(left, baseline - px(slot.formula.baseline)),
        size(px(slot.formula.width), px(slot.formula.height)),
    );
    window
        .paint_image(
            bounds,
            Corners::default(),
            slot.formula.image.clone(),
            0,
            false,
        )
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_math_uses_the_heading_size_and_a_separate_cache_entry() {
        for (level, size) in [(1, 30.), (2, 24.), (3, 20.), (4, 17.), (5, 17.), (6, 17.)] {
            let block = Block::new(
                BlockKind::Heading {
                    level,
                    id: String::new(),
                },
                0..0,
            );
            assert!(inline_math_shown(&block));
            assert_eq!(text_size(&block.kind), size);
            for zoom in [0.5, 1., 2.] {
                assert_ne!(
                    scale_key(text_size(&block.kind) * zoom),
                    scale_key(BODY_SIZE * zoom)
                );
            }
        }
        assert!(!inline_math_shown(&Block::new(BlockKind::Math, 0..0)));
    }
}
