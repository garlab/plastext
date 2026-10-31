use super::*;
use gpui::{Bounds, Size, TextRun, fill, font, point, rgb, size};

const STEP: f32 = 40.;
const MARGIN: f32 = 24.;

impl SgfView {
    pub(super) fn side_panel(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let cursor = self.cursor.as_ref();
        let mut info = div().flex().flex_col().gap_1();
        let mut comment = "No comment for this position.".to_owned();
        let mut comment_title = "Comment".to_owned();
        if let Some(c) = cursor {
            // Annotations change with the displayed board, not an unfinished request.
            let node = self.presented.as_ref().map_or(c.selected, |p| p.node());
            let game_id = c.document.nodes()[node].game;
            let root = c.document.games()[game_id].root;
            info = info.child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(clipped(c.document.title(game_id), 256)),
            );
            for (label, key) in [
                ("Black", "PB"),
                ("White", "PW"),
                ("Result", "RE"),
                ("Komi", "KM"),
                ("Date", "DT"),
            ] {
                if let Some(value) = c.document.text(root, key).filter(|s| !s.is_empty()) {
                    info = info.child(format!("{label}: {}", clipped(value, 256)));
                }
            }
            if let Some(p) = &self.presented {
                info = info.child(format!(
                    "Captures: Black {} · White {}",
                    p.captures[0], p.captures[1]
                ));
            }
            if let Some(name) = c.document.text(node, "N").filter(|s| !s.is_empty()) {
                comment_title = clipped(name, 80);
            }
            if let Some(text) = c
                .document
                .text(node, "C")
                .or_else(|| {
                    if node == root {
                        c.document.text(node, "GC")
                    } else {
                        None
                    }
                })
                .filter(|s| !s.is_empty())
            {
                comment = clipped(text, 16_384);
            }
        }
        div()
            .id("sgf-side-panel")
            .w(px(300. * self.zoom))
            .max_w(relative(0.45))
            .h_full()
            .flex_none()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_l_1()
            .border_color(theme.border)
            .text_size(px(13. * self.zoom))
            .child(
                div()
                    .id("sgf-game-info")
                    .h(relative(0.24))
                    .min_h_0()
                    .flex_none()
                    .overflow_y_scroll()
                    .p_3()
                    .child(info),
            )
            .child(
                div()
                    .h(relative(0.34))
                    .min_h_0()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(comment_title),
                    )
                    .child(
                        div()
                            .id("sgf-comment")
                            .track_scroll(&self.comment_scroll)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_3()
                            .pb_2()
                            .child(comment),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child("Game tree"),
                    )
                    .child(self.game_tree(theme, cx))
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .flex_none()
                            .text_size(px(11. * self.zoom))
                            .text_color(theme.text_muted)
                            .child("↑↓ moves · ←→ variations"),
                    ),
            )
            .into_any_element()
    }
    fn game_tree(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(cursor) = &self.cursor else {
            return div().flex_1().into_any_element();
        };
        let tree = cursor.tree.clone();
        let document = cursor.document.clone();
        let game = cursor.game();
        let zoom = self.zoom;
        let selected = cursor.selected;
        let path = cursor.path.clone();
        let content = graph_size(&tree.games[game], zoom);
        let family = self.style.font_family.clone();
        let view = cx.entity();
        div()
            .id("sgf-game-tree")
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.tree_scroll)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if let Some(c) = &this.cursor {
                        let local = event.position - this.tree_origin;
                        let column = ((f32::from(local.x) / this.zoom - MARGIN) / STEP).round();
                        let row = ((f32::from(local.y) / this.zoom - MARGIN) / STEP).round();
                        if row >= 0. && column >= 0. {
                            let center = center(row as usize, column as usize, this.zoom);
                            if (f32::from(local.x - center.x)).abs() <= 16. * this.zoom
                                && (f32::from(local.y - center.y)).abs() <= 16. * this.zoom
                            {
                                let node = c.tree.at(c.game(), row as usize, column as usize);
                                this.select(node, cx);
                            }
                        }
                    }
                    this.focus.focus(window);
                }),
            )
            .child(
                div()
                    .w(content.width)
                    .h(content.height)
                    .min_w_full()
                    .min_h_full()
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.tree_origin = bounds.origin;
                                    if this.reveal_tree {
                                        let viewport = this.tree_scroll.bounds().size;
                                        if viewport.width > px(0.) && viewport.height > px(0.) {
                                            this.reveal_tree = false;
                                            if let Some(c) = &this.cursor {
                                                let node = c.tree.nodes[c.selected];
                                                let target =
                                                    center(node.row, node.column, this.zoom);
                                                let current = this.tree_scroll.offset();
                                                let next = reveal(
                                                    current,
                                                    target,
                                                    viewport,
                                                    graph_size(&c.tree.games[c.game()], this.zoom),
                                                    px(18. * this.zoom),
                                                );
                                                if current != next {
                                                    this.tree_scroll.set_offset(next);
                                                    cx.notify();
                                                }
                                            }
                                        }
                                    }
                                });
                            },
                            move |bounds, _, window, cx| {
                                let clip = bounds.intersect(&window.content_mask().bounds);
                                if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
                                    return;
                                }
                                let rows = cells(
                                    clip.top() - bounds.top(),
                                    clip.bottom() - bounds.top(),
                                    zoom,
                                );
                                let columns = cells(
                                    clip.left() - bounds.left(),
                                    clip.right() - bounds.left(),
                                    zoom,
                                );
                                let visible = tree.visible(game, rows, columns);
                                let position = |id: NodeId| {
                                    let n = tree.nodes[id];
                                    bounds.origin + center(n.row, n.column, zoom)
                                };
                                let connector = |from, to, color, window: &mut Window| {
                                    let from = position(from);
                                    let to = position(to);
                                    let middle = (from.y + to.y) / 2.;
                                    let corners =
                                        [from, point(from.x, middle), point(to.x, middle), to];
                                    let stroke = px(1.5 * zoom);
                                    for pair in corners.windows(2) {
                                        let a = pair[0];
                                        let b = pair[1];
                                        let area = Bounds::new(
                                            point(a.x.min(b.x), a.y.min(b.y))
                                                - point(stroke / 2., stroke / 2.),
                                            size(
                                                (b.x - a.x).abs() + stroke,
                                                (b.y - a.y).abs() + stroke,
                                            ),
                                        )
                                        .intersect(&clip);
                                        if area.size.width > px(0.) && area.size.height > px(0.) {
                                            window.paint_quad(fill(area, color));
                                        }
                                    }
                                };
                                // The last child's connector keeps branch trunks visible even
                                // when some alternatives are beyond the horizontal viewport.
                                for &id in &visible {
                                    if let Some(last) = tree.nodes[id].last_child {
                                        connector(id, last, theme.text_muted, window);
                                    }
                                }
                                for &id in &visible {
                                    if let Some(parent) = document.nodes()[id].parent {
                                        connector(
                                            parent,
                                            id,
                                            if path[id] {
                                                rgb(0x6ca9e8)
                                            } else {
                                                theme.text_muted
                                            },
                                            window,
                                        );
                                    }
                                }
                                for id in visible {
                                    let node = tree.nodes[id];
                                    let at = position(id);
                                    let radius = px(13. * zoom);
                                    let outer = if id == selected {
                                        rgb(0x6ca9e8)
                                    } else if path[id] {
                                        theme.selection
                                    } else {
                                        theme.border
                                    };
                                    window.paint_quad(
                                        fill(
                                            Bounds::new(
                                                at - point(radius + px(3.), radius + px(3.)),
                                                size(
                                                    (radius + px(3.)) * 2.,
                                                    (radius + px(3.)) * 2.,
                                                ),
                                            ),
                                            outer,
                                        )
                                        .corner_radii(radius + px(3.)),
                                    );
                                    let (background, ink) = match node.color {
                                        Some(Color::Black) => (rgb(0x171717), rgb(0xffffff)),
                                        Some(Color::White) => (rgb(0xf4f3ee), rgb(0x171717)),
                                        None => (theme.surface, theme.text),
                                    };
                                    window.paint_quad(
                                        fill(
                                            Bounds::new(
                                                at - point(radius, radius),
                                                size(radius * 2., radius * 2.),
                                            ),
                                            background,
                                        )
                                        .corner_radii(
                                            if node.color.is_some() { radius } else { px(4.) },
                                        ),
                                    );
                                    let text = if node.color.is_some() {
                                        node.number.to_string()
                                    } else {
                                        "·".into()
                                    };
                                    let text_size =
                                        px((11_f32.min(40. / text.len() as f32)) * zoom);
                                    let run = TextRun {
                                        len: text.len(),
                                        font: font(family.clone()),
                                        color: ink.into(),
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    };
                                    let line = window.text_system().shape_line(
                                        text.into(),
                                        text_size,
                                        &[run],
                                        None,
                                    );
                                    let height = px(18. * zoom);
                                    line.paint(
                                        at - point(line.width / 2., height / 2.),
                                        height,
                                        window,
                                        cx,
                                    )
                                    .ok();
                                    if node.comment {
                                        let marker = px(3. * zoom);
                                        window.paint_quad(
                                            fill(
                                                Bounds::new(
                                                    at + point(radius - marker, -radius),
                                                    size(marker * 2., marker * 2.),
                                                ),
                                                rgb(0xe8b95d),
                                            )
                                            .corner_radii(marker),
                                        );
                                    }
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
            .into_any_element()
    }
}
fn center(row: usize, column: usize, zoom: f32) -> Point<Pixels> {
    point(
        px((MARGIN + column as f32 * STEP) * zoom),
        px((MARGIN + row as f32 * STEP) * zoom),
    )
}
fn graph_size(game: &tree::Game, zoom: f32) -> Size<Pixels> {
    size(
        px((MARGIN * 2. + game.columns.saturating_sub(1) as f32 * STEP) * zoom),
        px((MARGIN * 2. + game.rows.saturating_sub(1) as f32 * STEP) * zoom),
    )
}
fn cells(start: Pixels, end: Pixels, zoom: f32) -> std::ops::Range<usize> {
    let start = ((f32::from(start) / zoom - MARGIN) / STEP).floor().max(0.) as usize;
    let end = ((f32::from(end) / zoom - MARGIN) / STEP).ceil().max(0.) as usize + 1;
    start..end
}
fn reveal(
    offset: Point<Pixels>,
    target: Point<Pixels>,
    viewport: Size<Pixels>,
    content: Size<Pixels>,
    margin: Pixels,
) -> Point<Pixels> {
    let axis = |offset: Pixels, target: Pixels, viewport: Pixels, content: Pixels| {
        let position = offset + target;
        let next = if position < margin || position > viewport - margin {
            viewport / 2. - target
        } else {
            offset
        };
        next.clamp(-(content - viewport).max(px(0.)), px(0.))
    };
    point(
        axis(offset.x, target.x, viewport.width, content.width),
        axis(offset.y, target.y, viewport.height, content.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn following_a_move_reveals_both_axes_without_moving_visible_nodes() {
        let viewport = size(px(250.), px(200.));
        let content = size(px(1000.), px(1000.));
        assert_eq!(
            reveal(
                point(px(0.), px(0.)),
                point(px(24.), px(24.)),
                viewport,
                content,
                px(18.)
            ),
            point(px(0.), px(0.))
        );
        let offset = reveal(
            point(px(0.), px(0.)),
            point(px(500.), px(900.)),
            viewport,
            content,
            px(18.),
        );
        assert_eq!(offset, point(px(-375.), px(-800.)));
        assert_eq!(
            reveal(
                offset,
                point(px(500.), px(900.)),
                viewport,
                content,
                px(18.)
            ),
            offset
        );
    }
}
