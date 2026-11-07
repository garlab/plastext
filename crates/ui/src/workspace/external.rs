use super::*;
use plastext_buffer::DiskVersion;
use plastext_editor::{DiskChange, EditingDocument};
use plastext_platform::watch::{Batch, FileWatches};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(test)]
#[path = "external_tests.rs"]
mod tests;

pub(super) struct DiskState {
    watches: FileWatches,
    paths: Vec<PathBuf>,
    versions: HashMap<PathBuf, Option<DiskVersion>>,
    checking: bool,
    comparison: Option<Arc<AtomicBool>>,
    _events: gpui::Task<()>,
}

impl DiskState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let (watches, events) = FileWatches::new();
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                // The low-frequency metadata check also covers watcher overflow,
                // unavailable watches and files on filesystems with lost events.
                let event = events.recv();
                let timer = cx.background_executor().timer(Duration::from_secs(2));
                futures::pin_mut!(event, timer);
                let polled = match futures::future::select(event, timer).await {
                    futures::future::Either::Left((Ok(()), _)) => false,
                    futures::future::Either::Left((Err(_), _)) => break,
                    futures::future::Either::Right(_) => true,
                };
                if !polled {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
                if this
                    .update_in(cx, |this, _, cx| {
                        let mut batch = this.disk.watches.take();
                        batch.rescan |= polled;
                        this.observe_disk(batch, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            watches,
            paths: Vec::new(),
            versions: HashMap::new(),
            checking: false,
            comparison: None,
            _events: task,
        }
    }
}

impl Drop for DiskState {
    fn drop(&mut self) {
        if let Some(cancel) = &self.comparison {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workspace {
    fn show_disk_diff(
        &mut self,
        document: Entity<EditingDocument>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use plastext_buffer::{ByteSource, FileDocument};
        if self.disk.comparison.is_some() {
            return;
        }
        let Some(mine) = document.read(cx).version() else {
            return;
        };
        let Some(reviewed) = document.read(cx).reviewed_disk() else {
            return;
        };
        let path = reviewed.path().to_owned();
        let options = self.open_options;
        let style = self.text_style.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.disk.comparison = Some(cancel.clone());
        self.show_notice("Preparing comparison…", cx);
        let work = cx.background_executor().spawn(async move {
            let disk = FileDocument::open(&path, options, &cancel, |_, _| {})?;
            if disk.disk_version() != &reviewed {
                return Err(std::io::Error::other(
                    "File changed again; refresh the comparison",
                ));
            }
            let disk = disk.session().snapshot();
            let mut diff = None;
            if mine.text().len().max(disk.text().len()) <= 8 * 1024 * 1024 {
                let mut a = Vec::new();
                let mut b = Vec::new();
                disk.text().read_uncached(0..disk.text().len(), &mut a)?;
                mine.text().read_uncached(0..mine.text().len(), &mut b)?;
                let changes = plastext_diff::changes(&a, &b);
                let text = if changes.is_empty() {
                    "No text differences. File metadata changed.\n".to_owned()
                } else {
                    plastext_diff::unified(&a, &b, &changes)
                };
                let source = Arc::new(FileSource::from_bytes(text.into_bytes()));
                let index = LineIndex::for_source(source.clone())?;
                diff = Some((source, index));
            }
            Ok((disk, mine, diff))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            if this
                .update(cx, |this, cx| {
                    this.disk.comparison = None;
                    this.notice = None;
                    cx.notify();
                })
                .is_err()
            {
                cx.background_executor()
                    .spawn(async move {
                        drop(result);
                    })
                    .detach();
                return;
            }
            match result {
                Ok((disk, mine, diff)) => {
                    cx.open_window(Default::default(), |window, cx| {
                        window.set_window_title("Disk vs. Your Changes");
                        let (left, right) = if let Some((source, index)) = diff {
                            (
                                cx.new(|cx| TextView::new(source, index, style.clone(), None, cx)),
                                None,
                            )
                        } else {
                            (
                                cx.new(|cx| {
                                    TextView::from_snapshot(disk.text().clone(), style.clone(), cx)
                                }),
                                Some(cx.new(|cx| {
                                    TextView::from_snapshot(mine.text().clone(), style, cx)
                                })),
                            )
                        };
                        left.focus_handle(cx).focus(window);
                        cx.new(|_| Comparison { left, right })
                    })
                    .ok();
                    cx.background_executor()
                        .spawn(async move {
                            drop((disk, mine));
                        })
                        .detach();
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.show_notice(format!("Couldn't compare: {error}"), cx)
                    })
                    .ok();
                }
            }
        })
        .detach();
    }
    fn disk_documents(&self, cx: &App) -> Vec<Entity<EditingDocument>> {
        let mut docs: Vec<_> = self
            .panes
            .values()
            .flat_map(|p| &p.tabs)
            .filter_map(|tab| tab.view.editing_document(cx))
            .collect();
        docs.sort_by_key(Entity::entity_id);
        docs.dedup_by_key(|d| d.entity_id());
        docs
    }
    pub(super) fn update_disk_watches(&mut self, cx: &App) {
        let mut paths: Vec<_> = self
            .disk_documents(cx)
            .iter()
            .map(|d| d.read(cx).path().to_owned())
            .collect();
        paths.sort();
        paths.dedup();
        if paths != self.disk.paths {
            self.disk.versions.retain(|p, _| paths.contains(p));
            self.disk.watches.set_paths(paths.clone());
            self.disk.paths = paths;
        }
    }
    fn observe_disk(&mut self, batch: Batch, cx: &mut Context<Self>) {
        for document in self.disk_documents(cx) {
            let Some(expected) = document.read(cx).disk_version().or_else(|| {
                self.disk
                    .versions
                    .get(document.read(cx).path())
                    .cloned()
                    .flatten()
            }) else {
                continue;
            };
            let Some(destination) = batch.renames().find_map(|(from, to)| {
                expected.path().strip_prefix(from).ok().map(|relative| {
                    if relative.as_os_str().is_empty() {
                        to.to_owned()
                    } else {
                        to.join(relative)
                    }
                })
            }) else {
                continue;
            };
            let before = expected.clone();
            let work = cx.background_executor().spawn(async move {
                if before.path().try_exists().ok()? {
                    return None;
                }
                let current = DiskVersion::read(&destination).ok()?;
                before.same_file(&current).then_some(current)
            });
            cx.spawn(async move |_, cx| {
                if let Some(current) = work.await {
                    document
                        .update(cx, |doc, cx| {
                            doc.follow_rename(&expected, &current, cx);
                        })
                        .ok();
                }
            })
            .detach();
        }
        if self.disk.checking {
            return;
        }
        let jobs: Vec<_> = self
            .disk_documents(cx)
            .into_iter()
            .filter_map(|doc| {
                let path = doc.read(cx).path().to_owned();
                let disk = doc
                    .read(cx)
                    .disk_version()
                    .or_else(|| self.disk.versions.get(&path).cloned().flatten());
                if !batch.affects(&path)
                    && !disk.as_ref().is_some_and(|d| batch.affects(d.target()))
                {
                    return None;
                }
                let before = self
                    .disk
                    .versions
                    .get(&path)
                    .cloned()
                    .or_else(|| disk.clone().map(Some));
                Some((doc, path, before))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.disk.checking = true;
        let work = cx.background_executor().spawn(async move {
            jobs.into_iter()
                .map(|(doc, path, before)| {
                    let current = DiskVersion::read(&path);
                    (doc, path, before, current)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let results = work.await;
            this.update(cx, |this, cx| {
                this.disk.checking = false;
                for (doc, path, before, current) in results {
                    if doc.read(cx).path() != path {
                        continue;
                    }
                    let current = match current {
                        Ok(version) => Some(version),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(_) => {
                            doc.update(cx, |doc, cx| doc.external_changed(cx));
                            continue;
                        }
                    };
                    let changed = before
                        .as_ref()
                        .map_or(current.is_none(), |before| before != &current);
                    let target_changed = before
                        .as_ref()
                        .and_then(|v| v.as_ref())
                        .map(DiskVersion::target)
                        != current.as_ref().map(DiskVersion::target);
                    this.disk.versions.insert(path, current);
                    if target_changed {
                        this.disk.watches.set_paths(this.disk.paths.clone());
                    }
                    if changed {
                        doc.update(cx, |doc, cx| doc.load_external_version(cx));
                    }
                }
            })
            .ok();
        })
        .detach();
    }
    pub(super) fn render_disk_banner(
        &self,
        document: Entity<EditingDocument>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let change = document.read(cx).disk_change()?;
        let confirming = document.read(cx).confirming_overwrite();
        let reviewed = document.read(cx).reviewed_disk();
        let text = match (change, confirming) {
            (DiskChange::Missing, _) => {
                "File deleted or moved on disk. Your open document is kept."
            }
            (_, true) => "This file changed on disk. Overwrite it with your changes?",
            _ => "Changed on disk. Your unsaved changes are kept.",
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|d| d.bg(theme.border))
                .child(label)
        };
        let reload = document.clone();
        let keep = document.clone();
        let compare = document.clone();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .p_2()
                .bg(theme.surface)
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .child(div().flex_1().child(text))
                .when(change == DiskChange::Changed, |row| {
                    row.child(button("disk-diff", "Show Diff").on_click(cx.listener(
                        move |this, _, window, cx| this.show_disk_diff(compare.clone(), window, cx),
                    )))
                    .child(
                        button("disk-reload", "Reload (Discard Changes)").on_click(
                            move |_, _, cx| reload.update(cx, |doc, cx| doc.reload_from_disk(cx)),
                        ),
                    )
                })
                .when(confirming && reviewed.is_some(), |row| {
                    row.child(
                        button("disk-overwrite", "Overwrite").on_click(move |_, _, cx| {
                            document.update(cx, |doc, cx| {
                                doc.overwrite_disk(reviewed.clone().unwrap(), cx)
                            });
                        }),
                    )
                })
                .child(
                    button("disk-keep", if confirming { "Cancel" } else { "Keep Mine" })
                        .on_click(move |_, _, cx| keep.update(cx, |doc, cx| doc.keep_mine(cx))),
                )
                .into_any_element(),
        )
    }
}

struct Comparison {
    left: Entity<TextView>,
    right: Option<Entity<TextView>>,
}
impl Render for Comparison {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let panel = |label, view: Entity<TextView>| {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child(label),
                )
                .child(div().flex_1().min_h_0().child(view))
        };
        div()
            .key_context(KEY_CONTEXT)
            .on_action(|_: &CloseTab, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .size_full()
            .flex()
            .bg(theme.background)
            .text_color(theme.text)
            .child(panel(
                if self.right.is_some() {
                    "Disk (read-only snapshot)"
                } else {
                    "Disk → Your changes (read-only comparison)"
                },
                self.left.clone(),
            ))
            .children(
                self.right
                    .as_ref()
                    .map(|view| panel("Your changes (read-only snapshot)", view.clone())),
            )
    }
}
