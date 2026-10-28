//! Math, rendered off the UI thread and cached like images.
//!
//! Display formulas are blocks of their own. An inline formula takes the
//! place of its TeX in the text as a run of transparent `l`s about as wide
//! as the formula: the text wraps around it like a word, and the formula is
//! painted over it once the text is laid out.
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
    color: [u8; 4],
    /// Device pixels per logical pixel, in thousandths.
    scale: u32,
}

#[derive(Clone)]
pub(super) struct Formula {
    image: Arc<gpui::RenderImage>,
    /// Logical pixels.
    width: f32,
    height: f32,
    /// The baseline, in logical pixels from the top.
    baseline: f32,
}

#[derive(Default)]
pub(super) struct Formulas {
    cache: HashMap<FormulaKey, Option<Result<Formula, String>>>,
    order: VecDeque<FormulaKey>,
    /// The list rows showing each formula, to re-measure when it arrives.
    rows: HashMap<FormulaKey, Vec<usize>>,
    /// Measured for the current window and theme by [`MarkdownView::prepare_math`].
    inline: Option<InlineMetrics>,
}

impl Formulas {
    /// Forgets which rows show what, when the document's blocks change.
    pub(super) fn clear_rows(&mut self) {
        self.rows.clear();
    }
}

/// What inline formulas need from the window and theme, in logical pixels.
#[derive(Clone, Copy)]
struct InlineMetrics {
    /// The advance of [`SLOT_CHAR`] in the prose font.
    slot_char: f32,
    ascent: f32,
    descent: f32,
    line_height: f32,
    scale: f32,
    color: [u8; 4],
}

/// An inline formula in laid-out text.
pub(super) struct Slot {
    /// Where its placeholder starts in the displayed text.
    start: usize,
    /// How many [`SLOT_CHAR`]s it has.
    count: usize,
    /// The placeholder's width.
    width: f32,
    formula: Formula,
}

fn channels(color: gpui::Rgba) -> [u8; 4] {
    [color.r, color.g, color.b, color.a].map(|value| (value.clamp(0., 1.) * 255.).round() as u8)
}

fn scale_key(scale: f32) -> u32 {
    (scale * 1000.).round() as u32
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
        let scale = window.scale_factor();
        let color = channels(theme.text);
        // The prose font only changes on restart.
        if !self
            .formulas
            .inline
            .is_some_and(|metrics| metrics.scale == scale && metrics.color == color)
        {
            let font = text_run(1, false, false, false, theme.text, self).font;
            let text_system = window.text_system();
            let font_id = text_system.resolve_font(&font);
            let size = px(BODY_SIZE);
            self.formulas.inline =
                text_system
                    .advance(font_id, size, SLOT_CHAR)
                    .ok()
                    .map(|slot_char| InlineMetrics {
                        slot_char: f32::from(slot_char.width),
                        ascent: f32::from(text_system.ascent(font_id, size)),
                        // Font metrics give the descent as negative.
                        descent: f32::from(text_system.descent(font_id, size)).abs(),
                        line_height: BODY_SIZE * LINE_HEIGHT,
                        scale,
                        color,
                    });
        }
        if self.formulas.inline.is_none() || !inline_math_shown(block) {
            return;
        }
        for span in block.spans.iter().filter(|span| span.style.math) {
            let key = FormulaKey {
                tex: block.text[span.range.clone()].to_owned(),
                display: false,
                color: channels(theme.text),
                scale: scale_key(scale),
            };
            self.request_formula(&key, ix, scale, cx);
        }
    }

    /// The placeholder for an inline formula: how many [`SLOT_CHAR`]s, and
    /// how wide they are. `None` until the formula has rendered.
    pub(super) fn inline_slot(&self, tex: &str) -> Option<(usize, f32, Formula)> {
        let metrics = self.formulas.inline?;
        let key = FormulaKey {
            tex: tex.to_owned(),
            display: false,
            color: metrics.color,
            scale: scale_key(metrics.scale),
        };
        let formula = self.formulas.cache.get(&key)?.as_ref()?.as_ref().ok()?;
        let count = ((formula.width / metrics.slot_char).round() as usize).max(1);
        Some((count, count as f32 * metrics.slot_char, formula.clone()))
    }

    /// `text` with its inline formulas painted over their placeholders,
    /// with lines tall enough for the tallest one.
    pub(super) fn with_inline_math(
        &self,
        text: InteractiveText,
        layout: TextLayout,
        slots: Vec<Slot>,
    ) -> AnyElement {
        let Some(metrics) = self.formulas.inline.filter(|_| !slots.is_empty()) else {
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

    /// A display formula, or its TeX while it renders or when it can't.
    pub(super) fn display_math(
        &mut self,
        block: &Block,
        ix: usize,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = window.scale_factor();
        let key = FormulaKey {
            tex: block.text.to_string(),
            display: true,
            color: channels(theme.text),
            scale: scale_key(scale),
        };
        self.request_formula(&key, ix, scale, cx);
        let source = || {
            div().p_3().bg(theme.surface).child(self.rich_text(
                block,
                0..block.text.len(),
                ix,
                true,
                theme,
                cx,
            ))
        };
        match self.formulas.cache.get(&key).and_then(Option::as_ref) {
            Some(Ok(formula)) => div()
                .id(("md-math", ix))
                .w_full()
                .overflow_x_scroll()
                .child(
                    div().min_w_full().flex().justify_center().child(
                        gpui::img(formula.image.clone())
                            .flex_none()
                            .w(px(formula.width))
                            .h(px(formula.height)),
                    ),
                )
                .into_any_element(),
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
            async move {
                let px_per_em = BODY_SIZE * plastext_math::SCALE * scale;
                let rendered = plastext_math::render(&key.tex, key.display, px_per_em, key.color)?;
                let mut bgra = rendered.rgba;
                for pixel in bgra.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let buffer = image::RgbaImage::from_raw(rendered.width, rendered.height, bgra)
                    .ok_or("The formula's image has the wrong size.")?;
                Ok(Formula {
                    image: Arc::new(gpui::RenderImage::new([image::Frame::new(buffer)])),
                    width: rendered.width as f32 / scale,
                    height: rendered.height as f32 / scale,
                    baseline: rendered.baseline / scale,
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

/// Inline formulas are laid out at body size; headings and the TeX of
/// display math keep their source.
pub(super) fn inline_math_shown(block: &Block) -> bool {
    !matches!(block.kind, BlockKind::Heading { .. } | BlockKind::Math)
}

impl Slot {
    pub(super) fn new(start: usize, count: usize, width: f32, formula: Formula) -> Self {
        Self {
            start,
            count,
            width,
            formula,
        }
    }

    /// The placeholder's bytes in the displayed text.
    pub(super) fn range(&self) -> std::ops::Range<usize> {
        self.start..self.start + self.count * SLOT_CHAR.len_utf8()
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
    let left = after_first.x - px(slot_char) + px((slot.width - slot.formula.width) / 2.);
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
