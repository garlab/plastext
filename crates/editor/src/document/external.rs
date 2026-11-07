use super::*;
use plastext_buffer::ExternalState;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChange {
    Changed,
    Missing,
}

#[derive(Default)]
pub(super) struct External {
    pub pending_read: bool,
    pub change: Option<DiskChange>,
    pub reviewed: Option<DiskVersion>,
    pub hidden: bool,
    pub confirm_save: bool,
    pub overwrite: Option<DiskVersion>,
    running: bool,
    pub cancel: Option<Arc<AtomicBool>>,
    debounce: Option<Task<()>>,
}

impl EditingDocument {
    pub fn track_reading_position(
        &mut self,
        anchor: &mut Option<(plastext_buffer::PaneId, u64)>,
        offset: u64,
    ) -> Option<u64> {
        let session = self.file.as_mut()?.session_mut();
        let version = session.snapshot();
        if let Some((pane, generation)) = anchor {
            if *generation != version.generation() {
                *generation = version.generation();
                return session.viewport(*pane);
            }
            session.set_viewport(*pane, offset);
        } else {
            let offset = version
                .text()
                .clip_to_scalar(
                    offset.min(version.text().len()),
                    plastext_buffer::Bias::Before,
                )
                .ok()?;
            let pane = session
                .attach_pane(plastext_buffer::Selection::caret(offset))
                .ok()?;
            *anchor = Some((pane, version.generation()));
        }
        None
    }
    pub fn release_reading_position(&mut self, anchor: Option<(plastext_buffer::PaneId, u64)>) {
        if let Some((pane, _)) = anchor
            && let Some(file) = &mut self.file
        {
            file.session_mut().detach_pane(pane);
        }
    }
    pub fn disk_version(&self) -> Option<DiskVersion> {
        self.file.as_ref().map(|f| f.disk_version().clone())
    }
    pub fn follow_rename(
        &mut self,
        expected: &DiskVersion,
        current: &DiskVersion,
        cx: &mut Context<Self>,
    ) -> bool {
        if cx
            .global::<Documents>()
            .0
            .get(current.path())
            .and_then(WeakEntity::upgrade)
            .is_some()
        {
            return false;
        }
        if self.file_operation
            || self.preparing
            || self.is_saving()
            || !self.file.as_mut().map_or(
                self.path == expected.path() && expected.same_file(current),
                |file| file.follow_rename(expected, current),
            )
        {
            return false;
        }
        let old = std::mem::replace(&mut self.path, current.path().to_owned());
        cx.global_mut::<Documents>().0.remove(&old);
        let entity = cx.entity().downgrade();
        cx.global_mut::<Documents>()
            .0
            .insert(self.path.clone(), entity);
        self.load_external_version(cx);
        cx.notify();
        true
    }
    /// A preview/read-only view only needs an editing snapshot after an outside
    /// update. Keep initial preview opening on its existing streaming path.
    pub fn load_external_version(&mut self, cx: &mut Context<Self>) {
        if self.file.is_some() {
            self.external_changed(cx);
            return;
        }
        if self.preparing {
            self.external.pending_read = true;
            return;
        }
        self.preparing = true;
        let (path, options, cancel) = (self.path.clone(), self.options, self.cancel.clone());
        let preferences = self.preferences.clone();
        let work = cx.background_executor().spawn(async move {
            let file = FileDocument::open(&path, options, &cancel, |_, _| {})?;
            let writable = file.disk_version().is_writable()?;
            let version = file.session().snapshot();
            let mut sample = Vec::new();
            version
                .text()
                .read_uncached(0..version.text().len().min(65536), &mut sample)?;
            let preferences = plastext_settings::editing::for_file(
                &path,
                &String::from_utf8_lossy(&sample),
                &preferences,
            );
            Ok::<_, std::io::Error>((file, writable, preferences))
        });
        cx.spawn(async move |this, cx| {
            let mut ready = Some(work.await);
            this.update(cx, |this, cx| {
                this.preparing = false;
                if this.file.is_some() {
                    return;
                }
                match ready.take().unwrap() {
                    Ok((mut file, writable, preferences)) => {
                        file.session_mut().defer_history_drop();
                        this.file = Some(file);
                        this.writable = writable;
                        this.external.change = None;
                        match preferences {
                            Ok(preferences) => this.preferences = preferences,
                            Err(error) => {
                                this.notice =
                                    Some(format!("Couldn't load EditorConfig: {error}").into())
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        this.external.change = Some(DiskChange::Missing);
                        this.external.hidden = false;
                    }
                    Err(error) => this.notice = Some(format!("Couldn't reload: {error}").into()),
                }
                if let Some(offset) = this.edit_after_operation.take() {
                    this.begin_edit(offset, cx);
                }
                if this.external.pending_read {
                    this.external.pending_read = false;
                    this.load_external_version(cx);
                }
                cx.notify();
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(ready);
                })
                .detach();
        })
        .detach();
    }
    pub fn disk_change(&self) -> Option<DiskChange> {
        (!self.external.hidden)
            .then_some(self.external.change)
            .flatten()
    }
    pub fn reviewed_disk(&self) -> Option<DiskVersion> {
        self.external.reviewed.clone()
    }
    pub fn confirming_overwrite(&self) -> bool {
        self.external.confirm_save
    }
    pub fn keep_mine(&mut self, cx: &mut Context<Self>) {
        self.external.hidden = true;
        self.external.confirm_save = false;
        cx.notify();
    }
    /// A filesystem hint, including an event caused by our own atomic save.
    /// Coalesce bursts; never run overlapping snapshot copies for this document.
    pub fn external_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.external.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.external.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update(cx, |this, cx| this.check_external(false, cx))
                .ok();
        }));
    }
    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        self.check_external(true, cx);
    }
    pub fn overwrite_disk(&mut self, reviewed: DiskVersion, cx: &mut Context<Self>) {
        if self.external.reviewed.as_ref() != Some(&reviewed) || self.has_pending_input() {
            return;
        }
        self.external.overwrite = Some(reviewed);
        self.external.confirm_save = false;
        self.save(cx);
    }
    fn check_external(&mut self, discard: bool, cx: &mut Context<Self>) {
        if self.file.is_none() {
            return;
        }
        if self.external.running
            || self.preparing
            || self.is_saving()
            || self.file_operation
            || self.has_pending_input()
        {
            if discard {
                self.notice = Some("Finish the pending operation before reloading.".into());
                cx.notify();
            } else {
                self.external_changed(cx);
            }
            return;
        }
        let file = self.file.as_mut().unwrap();
        let job = if discard {
            file.reload_discarding_edits(self.options)
        } else {
            file.external_check(self.options)
        };
        self.external.running = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.external.cancel = Some(cancel.clone());
        let work = cx.background_executor().spawn({
            let cancel = cancel.clone();
            async move { job.run(&cancel, |_, _| {}) }
        });
        cx.spawn(async move |this, cx| {
            let mut observation = work.await;
            let mut retired = None;
            this.update(cx, |this, cx| {
                this.external.running = false;
                this.external.cancel = None;
                if cancel.load(Ordering::Relaxed) || this.has_pending_input() {
                    this.external_changed(cx);
                    return;
                }
                let reviewed = observation.disk_version().cloned();
                let Some(file) = &mut this.file else { return; };
                let result = file.finish_external(&mut observation);
                retired = result.retired;
                match result.state {
                    state @ (ExternalState::Unchanged | ExternalState::Reloaded) => {
                        file.session_mut().defer_history_drop();
                        this.external.change = None;
                        this.external.reviewed = None;
                        this.external.confirm_save = false;
                        if discard || state == ExternalState::Reloaded { this.notice = None; }
                    }
                    state @ (ExternalState::Conflict | ExternalState::Missing) => {
                        let change = if state == ExternalState::Missing { DiskChange::Missing } else { DiskChange::Changed };
                        if this.external.reviewed != reviewed || this.external.change != Some(change) {
                            this.external.hidden = false;
                        }
                        this.external.change = Some(change);
                        this.external.reviewed = reviewed;
                    }
                    ExternalState::Stale | ExternalState::Deferred => {
                        if discard { this.notice = Some("The document changed while reloading. Your newer edits were kept; choose Reload again to discard them.".into()); }
                        this.external_changed(cx);
                    }
                    ExternalState::Failed(error) => {
                        this.notice = Some(format!("Couldn't reload: {error}").into());
                    }
                }
                cx.notify();
            }).ok();
            cx.background_executor().spawn(async move { drop((observation, retired)); }).detach();
        }).detach();
    }
}
