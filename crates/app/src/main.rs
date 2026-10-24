//! The plastext application: startup, windows, single-instance server and session restore (spec §7.3).

use std::path::{self, PathBuf};

use clap::Parser as _;
use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use plastext_cli::Cli;
use plastext_ui::Workspace;

actions!(plastext, [Quit]);

fn main() {
    let cli = Cli::parse();
    let roots: Vec<PathBuf> = cli
        .locations
        .into_iter()
        .map(|location| path::absolute(&location.path).unwrap_or(location.path))
        .collect();

    Application::new().run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "plastext".into(),
            items: vec![MenuItem::action("Quit plastext", Quit)],
        }]);
        // Nothing can open a folder from the UI yet, so a windowless app is useless.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        if roots.is_empty() {
            open_window(None, cx);
        }
        for root in roots {
            open_window(Some(root), cx);
        }
        cx.activate(true);
    });
}

fn open_window(root: Option<PathBuf>, cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(Workspace::title(root.as_deref())),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1100.), px(720.)),
            cx,
        ))),
        ..Default::default()
    };
    if let Err(err) = cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::new(root, window, cx))
    }) {
        eprintln!("plastext: cannot open a window: {err:#}");
    }
}
