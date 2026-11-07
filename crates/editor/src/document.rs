//! One UI owner per path; disk work and bulk version retirement run on workers.
mod external;
#[cfg(test)]
mod external_tests;
mod preferences;
pub use external::DiskChange;

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task, WeakEntity};
use plastext_buffer::{ByteSource, DiskVersion, FileDocument, OpenOptions, SaveOptions};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
struct Documents(HashMap<PathBuf, WeakEntity<EditingDocument>>);
impl Global for Documents {}

#[derive(Default)]
pub(crate) struct Preferences(pub plastext_settings::Editor);
impl Global for Preferences {}

pub fn configure_editing(editor: plastext_settings::Editor, cx: &mut App) {
    cx.set_global(Preferences(editor));
}

pub struct EditingDocument {
    external: external::External,
    pub(crate) views: usize,
    pub(crate) pending_inputs: usize,
    pub(crate) file: Option<FileDocument>,
    pub(crate) active: bool,
    pub(crate) preparing: bool,
    writable: bool,
    pub(crate) notice: Option<SharedString>,
    pub(crate) preferences: plastext_settings::Editor,
    path: PathBuf,
    options: OpenOptions,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    file_operation: bool,
    save_preparing: bool,
    edit_after_operation: Option<u64>,
}
impl EditingDocument {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn shared(path: PathBuf, options: OpenOptions, cx: &mut App) -> Entity<Self> {
        cx.default_global::<Documents>();
        if let Some(existing) = cx
            .global::<Documents>()
            .0
            .get(&path)
            .and_then(WeakEntity::upgrade)
        {
            return existing;
        }
        let key = path.clone();
        let entity = cx.new(|cx| {
            cx.on_release(|this: &mut Self, cx| {
                this.cancel.store(true, Ordering::Relaxed);
                if let Some(cancel) = &this.external.cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                if let Some(file) = this.file.take() {
                    cx.background_executor()
                        .spawn(async move {
                            drop(file);
                        })
                        .detach();
                }
            })
            .detach();
            Self {
                external: external::External::default(),
                views: 0,
                pending_inputs: 0,
                file: None,
                active: false,
                preparing: false,
                writable: true,
                notice: None,
                preferences: cx
                    .try_global::<Preferences>()
                    .map(|p| p.0.clone())
                    .unwrap_or_default(),
                path,
                options,
                cancel: Arc::new(AtomicBool::new(false)),
                task: None,
                file_operation: false,
                save_preparing: false,
                edit_after_operation: None,
            }
        });
        let documents = &mut cx.global_mut::<Documents>().0;
        documents.retain(|_, value| value.upgrade().is_some());
        documents.insert(key, entity.downgrade());
        entity
    }
    pub fn is_dirty(&self) -> bool {
        self.pending_inputs != 0
            || self
                .file
                .as_ref()
                .is_some_and(|file| file.session().is_dirty())
    }
    /// Register a tab that owns an embedded source editor or a preview.
    pub fn retain_view(&mut self) {
        self.views += 1;
    }
    pub fn release_view(&mut self) {
        self.views = self.views.saturating_sub(1);
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.notice.clone()
    }
    pub fn view_count(&self) -> usize {
        self.views
    }
    pub fn has_pending_input(&self) -> bool {
        self.pending_inputs != 0
    }
    pub fn file_operation_pending(&self) -> bool {
        self.file_operation
    }
    pub fn is_preparing(&self) -> bool {
        self.preparing
    }
    pub fn set_file_operation_pending(&mut self, pending: bool, cx: &mut Context<Self>) {
        self.file_operation = pending;
        if !pending && let Some(offset) = self.edit_after_operation.take() {
            self.begin_edit(offset, cx);
        }
        cx.notify();
    }
    pub fn all(cx: &App) -> Vec<Entity<Self>> {
        cx.try_global::<Documents>()
            .map(|documents| {
                documents
                    .0
                    .values()
                    .filter_map(WeakEntity::upgrade)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn version(&self) -> Option<plastext_buffer::DocumentSnapshot> {
        self.file.as_ref().map(|file| file.session().snapshot())
    }
    pub fn is_current(&self, version: &plastext_buffer::DocumentSnapshot) -> bool {
        self.file
            .as_ref()
            .is_some_and(|file| file.session().is_current(version))
    }
    pub fn is_saving(&self) -> bool {
        self.save_preparing || self.file.as_ref().is_some_and(FileDocument::is_saving)
    }
    pub fn is_editing(&self) -> bool {
        self.active
    }
    pub fn begin_edit(&mut self, offset: u64, cx: &mut Context<Self>) {
        if self.file_operation {
            // Switching Preview → Source during a rename/move must not lose
            // the request to edit once the operation releases this document.
            self.edit_after_operation = Some(offset);
            return;
        }
        if self.file.is_some() {
            self.active = self.writable;
            if !self.writable {
                self.notice = Some("Read-only file".into());
            }
            cx.notify();
            return;
        }
        if self.preparing {
            self.edit_after_operation = Some(offset);
            return;
        }
        self.preparing = true;
        self.notice = Some("Preparing edit mode…".into());
        let (path, options, cancel) = (self.path.clone(), self.options, self.cancel.clone());
        let preferences = self.preferences.clone();
        let load = cx.background_executor().spawn(async move {
            if !DiskVersion::read(&path)?.is_writable()? {
                return Ok(None);
            }
            let mut file = FileDocument::open(&path, options, &cancel, |_, _| {})?;
            file.session_mut().defer_history_drop();
            let snapshot = file.session().snapshot();
            let end = snapshot.text().len();
            snapshot.text().load(
                offset.saturating_sub(1024 * 1024).min(end)
                    ..offset.saturating_add(1024 * 1024).min(end),
            )?;
            let mut sample = Vec::new();
            snapshot
                .text()
                .read_uncached(0..end.min(65536), &mut sample)?;
            let preferences = plastext_settings::editing::for_file(
                &path,
                &String::from_utf8_lossy(&sample),
                &preferences,
            );
            Ok::<_, std::io::Error>(Some((file, preferences)))
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let mut result = Some(result);
            this.update(cx, |this, cx| {
                this.preparing = false;
                match result.take().unwrap() {
                    Ok(Some((file, preferences))) => {
                        this.edit_after_operation = None;
                        this.file = Some(file);
                        this.active = true;
                        this.notice = match preferences {
                            Ok(preferences) => {
                                this.preferences = preferences;
                                None
                            }
                            Err(error) => {
                                Some(format!("Couldn't load EditorConfig: {error}").into())
                            }
                        };
                    }
                    Ok(None) => {
                        this.active = false;
                        this.notice = Some("Read-only file".into());
                    }
                    Err(error) => {
                        this.notice = Some(format!("Couldn't enter edit mode: {error}").into())
                    }
                }
                if this.external.pending_read {
                    this.external.pending_read = false;
                    this.load_external_version(cx);
                }
                cx.notify();
            })
            .ok();
            // If the last view closed while preparing, retire the result here.
            cx.background_executor()
                .spawn(async move {
                    drop(result);
                })
                .detach();
        }));
        cx.notify();
    }
    pub fn leave_edit(&mut self, cx: &mut Context<Self>) {
        if self.is_dirty() {
            self.notice = Some("Save or undo your changes before returning to read mode.".into());
        } else {
            self.active = false;
            self.notice = None;
        }
        cx.notify();
    }
    pub(crate) fn retire_versions(&mut self, cx: &mut Context<Self>) {
        if let Some(retired) = self
            .file
            .as_mut()
            .and_then(|file| file.session_mut().take_retired_history())
        {
            cx.background_executor()
                .spawn(async move {
                    drop(retired);
                })
                .detach();
        }
    }
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        self.retire_versions(cx);
        self.notice = None;
        cx.notify();
    }
    pub fn reopen_encoding(&mut self, label: &str, cx: &mut Context<Self>) {
        if self.is_dirty() || self.is_saving() || self.file_operation {
            return;
        }
        let hint = if label == "auto" {
            plastext_buffer::EncodingHint::Auto
        } else if let Some(encoding) = plastext_buffer::TextEncoding::for_label(label) {
            plastext_buffer::EncodingHint::Explicit(encoding)
        } else {
            return;
        };
        let Some(job) = self
            .file
            .as_mut()
            .and_then(|file| file.reopen_with_encoding(hint, self.options))
        else {
            return;
        };
        self.preparing = true;
        let cancel = self.cancel.clone();
        let work = cx
            .background_executor()
            .spawn(async move { job.run(&cancel, |_, _| {}) });
        cx.spawn(async move |this, cx| {
            let mut observation = work.await;
            let retired = this.update(cx, |this, cx| {
                this.preparing = false;
                let result = this.file.as_mut().map(|file| {
                    let result = file.finish_external(&mut observation);
                    file.session_mut().defer_history_drop();
                    result
                });
                this.notice = result.as_ref().and_then(|result| match &result.state {
                    plastext_buffer::ExternalState::Reloaded => None,
                    other => Some(format!("Couldn't reopen with encoding: {other:?}").into()),
                });
                cx.notify();
                result
            });
            cx.background_executor()
                .spawn(async move { drop((observation, retired)) })
                .detach();
        })
        .detach();
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.file_operation || self.is_saving() {
            return;
        }
        if self.external.change.is_some() && self.external.overwrite.is_none() {
            self.external.confirm_save = true;
            self.external.hidden = false;
            cx.notify();
            return;
        }
        let options = self.preferences.clone();
        if !options.trim_trailing_whitespace
            && options.insert_final_newline.is_none()
            && options.charset.is_none()
            && options.end_of_line.is_none()
        {
            self.save_now(cx);
            return;
        }
        let Some(file) = &mut self.file else { return };
        let session = file.session_mut();
        let Ok(pane) = session.attach_pane(plastext_buffer::Selection::caret(0)) else {
            return;
        };
        let base = session.pane_snapshot(pane).unwrap();
        self.save_preparing = true;
        self.notice = Some("Preparing save…".into());
        let cancel = self.cancel.clone();
        let work = cx.background_executor().spawn(async move {
            let result = preferences::prepare(base.document().text(), &options, &cancel);
            (base, result)
        });
        cx.spawn(async move |this, cx| {
            let (base, result) = work.await;
            this.update(cx, |this, cx| {
                this.save_preparing = false;
                let result = result.and_then(|(prepared, format)| {
                    let session = this
                        .file
                        .as_mut()
                        .ok_or("Document is closed")?
                        .session_mut();
                    session
                        .apply_prepared(&base, prepared, plastext_buffer::UndoGroup::New)
                        .map_err(|_| "Document changed while preparing save; press Save again")?;
                    session
                        .set_format(&session.snapshot(), format)
                        .map_err(|e| format!("{e:?}"))?;
                    Ok(())
                });
                if let Some(file) = &mut this.file {
                    file.session_mut().detach_pane(pane);
                }
                this.changed(cx);
                match result {
                    Ok(()) => this.save_now(cx),
                    Err(error) => this.notice = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn save_now(&mut self, cx: &mut Context<Self>) {
        if self.file_operation {
            return;
        }
        let overwrite = self.external.overwrite.take();
        let Some(file) = &mut self.file else {
            return;
        };
        let job = {
            let options = SaveOptions {
                atomic: self.preferences.atomic_save,
            };
            if let Some(reviewed) = overwrite {
                file.begin_overwrite(reviewed, options)
            } else {
                Ok(file.begin_save(options))
            }
        };
        let job = match job {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(error) => {
                self.notice = Some(format!("Couldn't overwrite: {}", error.message).into());
                self.external_changed(cx);
                cx.notify();
                return;
            }
        };
        self.notice = Some("Saving…".into());
        let cancel = self.cancel.clone();
        let task = cx
            .background_executor()
            .spawn(async move { job.run(&cancel, |_| {}) });
        cx.spawn(async move |this, cx| {
            let completion = task.await;
            this.update(cx, |this, cx| {
                if let Some(file) = &mut this.file {
                    this.notice = file.finish_save(&completion).err().map(|error| {
                        let recovery = error
                            .recovery_path
                            .as_ref()
                            .map(|path| format!(" Recovery copy: {}", path.display()))
                            .unwrap_or_default();
                        format!("Couldn't save: {}{recovery}", error.message).into()
                    });
                }
                this.external.change = None;
                this.external.reviewed = None;
                this.external.confirm_save = false;
                this.external_changed(cx);
                cx.notify();
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(completion);
                })
                .detach();
        })
        .detach();
        cx.notify();
    }
}
