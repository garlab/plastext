//! One UI owner per path; disk work and bulk version retirement run on workers.
use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task, WeakEntity};
use plastext_buffer::{ByteSource, FileDocument, OpenOptions, SaveOptions};
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

pub struct EditingDocument {
    pub(crate) views: usize,
    pub(crate) pending_inputs: usize,
    pub(crate) file: Option<FileDocument>,
    pub(crate) active: bool,
    pub(crate) preparing: bool,
    pub(crate) notice: Option<SharedString>,
    path: PathBuf,
    options: OpenOptions,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}
impl EditingDocument {
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
                views: 0,
                pending_inputs: 0,
                file: None,
                active: false,
                preparing: false,
                notice: None,
                path,
                options,
                cancel: Arc::new(AtomicBool::new(false)),
                task: None,
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
    pub fn view_count(&self) -> usize {
        self.views
    }
    pub fn has_pending_input(&self) -> bool {
        self.pending_inputs != 0
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
        self.file.as_ref().is_some_and(FileDocument::is_saving)
    }
    pub fn is_editing(&self) -> bool {
        self.active
    }
    pub fn begin_edit(&mut self, offset: u64, cx: &mut Context<Self>) {
        if self.file.is_some() {
            self.active = true;
            cx.notify();
            return;
        }
        if self.preparing {
            return;
        }
        self.preparing = true;
        self.notice = Some("Preparing edit mode…".into());
        let (path, options, cancel) = (self.path.clone(), self.options, self.cancel.clone());
        let load = cx.background_executor().spawn(async move {
            let mut file = FileDocument::open(&path, options, &cancel, |_, _| {})?;
            file.session_mut().defer_history_drop();
            let snapshot = file.session().snapshot();
            let end = snapshot.text().len();
            snapshot.text().load(
                offset.saturating_sub(1024 * 1024).min(end)
                    ..offset.saturating_add(1024 * 1024).min(end),
            )?;
            Ok::<_, std::io::Error>(file)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let mut result = Some(result);
            this.update(cx, |this, cx| {
                this.preparing = false;
                match result.take().unwrap() {
                    Ok(file) => {
                        this.file = Some(file);
                        this.active = true;
                        this.notice = None;
                    }
                    Err(error) => {
                        this.notice = Some(format!("Couldn't enter edit mode: {error}").into())
                    }
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
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
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
        self.notice = None;
        cx.notify();
    }
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self
            .file
            .as_mut()
            .and_then(|file| file.begin_save(SaveOptions::default()))
        else {
            return;
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
