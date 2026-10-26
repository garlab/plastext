//! Display math, rendered off the UI thread and cached like images.
use super::*;

/// Formulas rendering at once, and kept rendered.
const MAX_PENDING: usize = 4;
const MAX_CACHED: usize = 256;

/// Everything a rendered formula depends on.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct FormulaKey {
    tex: String,
    display: bool,
    color: [u8; 4],
    /// Device pixels per logical pixel, in thousandths.
    scale: u32,
}

pub(super) struct Formula {
    image: Arc<gpui::RenderImage>,
    /// Logical pixels.
    width: f32,
    height: f32,
}

#[derive(Default)]
pub(super) struct Formulas {
    cache: HashMap<FormulaKey, Option<Result<Formula, String>>>,
    order: VecDeque<FormulaKey>,
    /// The list rows showing each formula, to re-measure when it arrives.
    rows: HashMap<FormulaKey, Vec<usize>>,
}

impl Formulas {
    /// Forgets which rows show what, when the document's blocks change.
    pub(super) fn clear_rows(&mut self) {
        self.rows.clear();
    }
}

impl MarkdownView {
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
        let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        let key = FormulaKey {
            tex: block.text.to_string(),
            display: true,
            color: [
                channel(theme.text.r),
                channel(theme.text.g),
                channel(theme.text.b),
                channel(theme.text.a),
            ],
            scale: (scale * 1000.).round() as u32,
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
