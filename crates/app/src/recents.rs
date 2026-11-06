//! Serialize history updates on a worker; no filesystem work in rendering.
use futures::{StreamExt, channel::mpsc};
use gpui::{App, Global};
use plastext_settings::RecentFolders;
use std::path::PathBuf;

enum Update {
    Opened(PathBuf),
    Clear,
}
struct History(mpsc::UnboundedSender<Update>);
impl Global for History {}

pub(super) fn init(cx: &mut App) {
    init_at(RecentFolders::path(), cx);
}

fn init_at(path: Option<PathBuf>, cx: &mut App) {
    let (sender, mut updates) = mpsc::unbounded();
    cx.set_global(History(sender));
    let load_path = path.clone();
    let load = cx.background_executor().spawn(async move {
        load_path
            .as_deref()
            .map(RecentFolders::load)
            .transpose()
            .unwrap_or_else(|error| {
                eprintln!("Couldn't read recent folders: {error}");
                None
            })
            .unwrap_or_default()
    });
    cx.spawn(async move |cx| {
        let mut history = load.await;
        loop {
            let folders = history.folders().to_vec();
            if cx
                .update(|cx| cx.set_global(plastext_ui::RecentFolders(folders)))
                .is_err()
            {
                break;
            }
            let Some(update) = updates.next().await else {
                break;
            };
            let path = path.clone();
            history = cx
                .background_executor()
                .spawn(async move {
                    let changed = match update {
                        Update::Opened(root) => std::fs::canonicalize(root)
                            .ok()
                            .filter(|root| root.is_dir())
                            .is_some_and(|root| history.record(root)),
                        Update::Clear => {
                            history.clear();
                            true
                        }
                    };
                    if changed
                        && let Some(path) = path
                        && let Err(error) = history.save(&path)
                    {
                        eprintln!("Couldn't save recent folders: {error}");
                    }
                    history
                })
                .await;
        }
    })
    .detach();
}

pub(super) fn record(root: Option<&std::path::Path>, cx: &App) {
    if let Some(root) = root
        && let Some(history) = cx.try_global::<History>()
    {
        history
            .0
            .unbounded_send(Update::Opened(root.to_owned()))
            .ok();
    }
}

pub(super) fn clear(cx: &App) {
    if let Some(history) = cx.try_global::<History>() {
        history.0.unbounded_send(Update::Clear).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn queued_opens_survive_loading_and_clearing_is_persisted(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("recent-folders.json");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let mut history = RecentFolders::default();
        history.record(first.canonicalize().unwrap());
        history.save(&file).unwrap();
        cx.update(|cx| {
            init_at(Some(file.clone()), cx);
            record(Some(&second), cx);
            record(Some(&first), cx);
        });
        cx.run_until_parked();
        let expected = vec![
            first.canonicalize().unwrap(),
            second.canonicalize().unwrap(),
        ];
        assert_eq!(
            cx.update(|cx| cx.global::<plastext_ui::RecentFolders>().0.clone()),
            expected
        );
        assert_eq!(RecentFolders::load(&file).unwrap().folders(), expected);
        cx.update(|cx| clear(cx));
        cx.run_until_parked();
        assert!(RecentFolders::load(&file).unwrap().folders().is_empty());
    }
}
