use super::*;

impl MarkdownView {
    pub(super) fn metadata_block(
        &self,
        block: &Block,
        ix: usize,
        theme: Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        match &block.kind {
            BlockKind::FrontMatter { format, error } => {
                let section = block.source.start;
                let expanded = self.expanded.contains(&section);
                div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.surface)
                    .text_sm()
                    .child(
                        div()
                            .id(("md-metadata", ix))
                            .cursor_pointer()
                            .text_color(theme.text_muted)
                            .child(format!(
                                "{} Front matter · {}{}",
                                if expanded { "▾" } else { "▸" },
                                format.label(),
                                if error.is_some() { " · source" } else { "" }
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_metadata(section, cx);
                            })),
                    )
                    .when(expanded, |d| {
                        d.children(error.as_ref().map(|message| {
                            div()
                                .mt_2()
                                .text_color(theme.text_muted)
                                .child(message.clone())
                        }))
                    })
                    .into_any_element()
            }
            BlockKind::FrontMatterRow { key, value, .. } => {
                div()
                    .flex()
                    .w_full()
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(div().w(relative(0.3)).flex_none().min_w_0().p_2().child(
                        self.rich_text(block, key.clone(), ix.wrapping_mul(4096), true, theme, cx),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_2()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(self.rich_text(
                                block,
                                value.clone(),
                                ix.wrapping_mul(4096) + 1,
                                true,
                                theme,
                                cx,
                            )),
                    )
                    .into_any_element()
            }
            BlockKind::FrontMatterSource { .. } => div()
                .p_3()
                .bg(theme.surface)
                .text_sm()
                .child(self.rich_text(block, 0..block.text.len(), ix, true, theme, cx))
                .into_any_element(),
            _ => unreachable!("metadata renderer requires a metadata block"),
        }
    }
}
