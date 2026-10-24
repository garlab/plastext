//! Go to symbol (spec NAV-4): the outline of the current file, filtered by a fuzzy query.

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use nucleo::{
    Config, Matcher,
    pattern::{CaseMatching, Normalization, Pattern},
};
use plastext_editor::{TextInput, TextInputEvent, Theme};
use plastext_syntax::Symbol;

actions!(
    symbol_picker,
    [SelectNext, SelectPrevious, Confirm, Dismiss]
);

const KEY_CONTEXT: &str = "SymbolPicker";
const WIDTH: f32 = 560.;
const ROW_HEIGHT: f32 = 28.;
const VISIBLE_ROWS: usize = 14;
const INDENT: f32 = 16.;

pub fn init(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("ctrl-n", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("ctrl-p", SelectPrevious, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("escape", Dismiss, context),
    ]);
}

pub enum SymbolPickerEvent {
    /// Go to the symbol starting at this byte offset.
    Go(u64),
    Dismissed,
}

pub struct SymbolPicker {
    symbols: Arc<Vec<Symbol>>,
    input: Entity<TextInput>,
    /// Indices into `symbols` that match the query, best first.
    shown: Vec<usize>,
    /// Whether `shown` is filtered, rather than the whole outline in order.
    filtered: bool,
    selected: usize,
    scroll_handle: UniformListScrollHandle,
    _input_changes: Subscription,
}

impl EventEmitter<SymbolPickerEvent> for SymbolPicker {}

impl SymbolPicker {
    /// A picker over `symbols`, starting on the last one before `position`.
    pub fn new(
        symbols: Arc<Vec<Symbol>>,
        position: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new("Go to symbol", cx));
        let input_changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
            this.filter(cx)
        });
        input.focus_handle(cx).focus(window);
        let position = position as usize;
        let selected = symbols
            .iter()
            .rposition(|symbol| symbol.range.start <= position)
            .unwrap_or(0);
        let scroll_handle = UniformListScrollHandle::new();
        scroll_handle.scroll_to_item(selected, ScrollStrategy::Center);
        Self {
            shown: (0..symbols.len()).collect(),
            filtered: false,
            symbols,
            input,
            selected,
            scroll_handle,
            _input_changes: input_changes,
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text().trim().to_owned();
        self.filtered = !query.is_empty();
        self.shown = if self.filtered {
            let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
            let names = self
                .symbols
                .iter()
                .enumerate()
                .map(|(index, symbol)| Name(index, &symbol.name));
            pattern
                .match_list(names, &mut Matcher::new(Config::DEFAULT))
                .into_iter()
                .map(|(Name(index, _), _)| index)
                .collect()
        } else {
            (0..self.symbols.len()).collect()
        };
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.shown.is_empty() {
            self.select((self.selected + 1) % self.shown.len(), cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        if !self.shown.is_empty() {
            self.select(
                (self.selected + self.shown.len() - 1) % self.shown.len(),
                cx,
            );
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        self.scroll_handle
            .scroll_to_item(index, ScrollStrategy::Top);
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.go(self.selected, cx);
    }

    fn go(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(&symbol) = self.shown.get(index) {
            cx.emit(SymbolPickerEvent::Go(
                self.symbols[symbol].range.start as u64,
            ));
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SymbolPickerEvent::Dismissed);
    }
}

/// A symbol name with its index, for matching.
struct Name<'a>(usize, &'a str);

impl AsRef<str> for Name<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

impl Focusable for SymbolPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for SymbolPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let count = self.shown.len();
        let results = if count == 0 {
            div()
                .h(px(ROW_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .text_color(theme.text_muted)
                .child("No matching symbols")
                .into_any_element()
        } else {
            uniform_list(
                "symbols",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|index| {
                            let symbol = &this.symbols[this.shown[index]];
                            let indent = if this.filtered {
                                0.
                            } else {
                                INDENT * symbol.depth as f32
                            };
                            div()
                                .id(index)
                                .h(px(ROW_HEIGHT))
                                .pl(px(12. + indent))
                                .pr_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .when(index == this.selected, |row| row.bg(theme.selection))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(theme.text)
                                        .child(symbol.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(symbol.kind.clone()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.go(index, cx)))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.scroll_handle.clone())
            .h(px(ROW_HEIGHT * count.min(VISIBLE_ROWS) as f32))
            .into_any_element()
        };
        div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(SymbolPickerEvent::Dismissed)))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .text_sm()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.input.clone()),
            )
            .child(results)
    }
}
