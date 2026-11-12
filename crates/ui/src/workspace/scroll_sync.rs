//! Event-driven linking of visible views. Only the pane the user operates leads;
//! notifications from followers never turn rounding or clamping into feedback.
use super::*;
use crate::scroll_sync::{Position, Revision};
use gpui::EntityId;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct State {
    pub enabled: bool,
    leader: Option<usize>,
    queued: bool,
    sent: HashMap<EntityId, (EntityId, Revision, Position)>,
}

pub(super) fn observe_keys(cx: &mut Context<Workspace>) -> Subscription {
    let weak = cx.entity().downgrade();
    // Bound shortcuts bypass element key handlers. Observe before dispatch so
    // creating a split does not accidentally make the new, still-loading pane
    // lead its already-positioned peers.
    cx.intercept_keystrokes(move |_, window, cx| {
        weak.update(cx, |this, cx| {
            if !this.scroll_sync.enabled {
                return;
            }
            let pane = this.panes.iter().find_map(|(&id, pane)| {
                pane.active_tab()
                    .filter(|tab| tab.view.focus_handle(cx).contains_focused(window, cx))
                    .map(|_| id)
            });
            if let Some(pane) = pane {
                this.lead_scroll(pane, cx);
            }
        })
        .ok();
    })
}

impl Workspace {
    pub(super) fn toggle_scroll_sync(
        &mut self,
        _: &ToggleScrollSync,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scroll_sync.enabled = !self.scroll_sync.enabled;
        self.scroll_sync.leader = Some(self.active_pane);
        self.scroll_sync.sent.clear();
        for tab in self.panes.values().flat_map(|pane| &pane.tabs) {
            tab.view.cancel_scroll_sync(cx);
        }
        self.queue_scroll_sync(cx);
        cx.notify();
    }

    pub(super) fn lead_scroll(&mut self, pane: usize, cx: &mut Context<Self>) {
        if self.scroll_sync.enabled {
            if let Some(tab) = self.panes.get(&pane).and_then(Pane::active_tab) {
                tab.view.cancel_scroll_sync(cx);
            }
            self.scroll_sync.leader = Some(pane);
            self.queue_scroll_sync(cx);
        }
    }

    pub(super) fn queue_scroll_sync(&mut self, cx: &mut Context<Self>) {
        if !self.scroll_sync.enabled || self.scroll_sync.queued {
            return;
        }
        self.scroll_sync.queued = true;
        let weak = cx.entity().downgrade();
        cx.defer(move |cx| {
            weak.update(cx, |this, cx| {
                this.scroll_sync.queued = false;
                this.synchronize_scroll(cx);
            })
            .ok();
        });
    }

    fn synchronize_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.scroll_sync.enabled {
            return;
        }
        let leader = self.scroll_sync.leader.unwrap_or(self.active_pane);
        let Some(tab) = self.panes.get(&leader).and_then(Pane::active_tab) else {
            return;
        };
        let Some(position) = tab.view.scroll_position(cx) else {
            return;
        };
        let source_id = tab.view.to_any().entity_id();
        let document = tab.view.editing_document(cx);
        let path = &tab.path;
        let followers: Vec<_> = self
            .panes
            .iter()
            .filter_map(|(&id, pane)| {
                let tab = pane.active_tab()?;
                if id == leader
                    || (tab.path != *path
                        && (document.is_none() || tab.view.editing_document(cx) != document))
                {
                    return None;
                }
                let revision = tab.view.scroll_revision(cx)?;
                Some((tab.view.clone(), revision))
            })
            .collect();
        // Retain only visible followers; showing a hidden tab re-applies the
        // leader's position even if it has not moved in the meantime.
        self.scroll_sync.sent.retain(|id, _| {
            followers
                .iter()
                .any(|(view, _)| view.to_any().entity_id() == *id)
        });
        for (view, revision) in followers {
            let id = view.to_any().entity_id();
            let update = (source_id, revision, position);
            if self.scroll_sync.sent.get(&id) != Some(&update) {
                self.scroll_sync.sent.insert(id, update);
                view.sync_scroll(position, cx);
            }
        }
    }
}
