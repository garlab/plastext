//! Blink only an idle, focused editing caret. No animation-frame polling.
use super::*;

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub(super) struct Caret {
    pub(super) visible: bool,
    pub(super) task: Option<Task<()>>,
    observers: Vec<Subscription>,
}

impl Default for Caret {
    fn default() -> Self {
        Self {
            visible: true,
            task: None,
            observers: Vec::new(),
        }
    }
}

impl TextView {
    pub(super) fn reset_caret_blink(&mut self, cx: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing {
            editing.caret.task = None;
            editing.caret.visible = true;
            cx.notify();
        }
    }

    fn can_blink_caret(&self, window: &Window) -> bool {
        window.is_window_active()
            && self.focus_handle.is_focused(window)
            && !self.selecting
            && self.selection.is_some()
            && self.editing.as_ref().is_some_and(|editing| {
                editing.active
                    && editing.pane.is_some()
                    && editing.marked.is_none()
                    && editing.queue.is_empty()
            })
    }

    pub(in crate::text_view) fn sync_caret_blink(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        if editing.caret.observers.is_empty() {
            editing.caret.observers = vec![
                cx.on_focus(&self.focus_handle, window, |this, _, cx| {
                    this.reset_caret_blink(cx);
                }),
                cx.on_blur(&self.focus_handle, window, |this, _, cx| {
                    this.reset_caret_blink(cx);
                }),
                cx.observe_window_activation(window, |this, _, cx| {
                    this.reset_caret_blink(cx);
                }),
            ];
        }
        if !self.can_blink_caret(window) {
            let caret = &mut self.editing.as_mut().unwrap().caret;
            caret.task = None;
            caret.visible = true;
        } else if self.editing.as_ref().unwrap().caret.task.is_none() {
            self.editing.as_mut().unwrap().caret.task =
                Some(cx.spawn_in(window, async move |this, cx| {
                    loop {
                        cx.background_executor().timer(BLINK_INTERVAL).await;
                        let keep_blinking = this.update_in(cx, |this, window, cx| {
                            if !this.can_blink_caret(window) {
                                this.reset_caret_blink(cx);
                                return false;
                            }
                            let caret = &mut this.editing.as_mut().unwrap().caret;
                            caret.visible = !caret.visible;
                            cx.notify();
                            true
                        });
                        if !matches!(keep_blinking, Ok(true)) {
                            break;
                        }
                    }
                }));
        }
    }
}
